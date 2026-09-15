// ============================================================================
// 自动注册（等待注册态兜底）
//
// 场景：本地无可自动转世角色（全新安装 / 全部归隐 / auto_rebirth 关闭）时，
// 进程进入等待注册态。此模块提供统一的倒计时布防与超时自动生成角色能力：
//   - 面板引导人工注册（setup/status 暴露剩余秒数，面板渲染倒计时）
//   - 超时兜底自动生成（loopback 调用自身 HTTP API，复用 generate/register
//     handler 的全部行为：schema 校验 / 401 刷新 / character.yaml 落盘 /
//     reconnect_tx 广播）
//
// 两条等待路径共用本模块：
//   1. 冷启动（bin/cyber-jianghu-agent.rs::await_character_loop，Agent 构建前）
//   2. 热等待（core/reconnect.rs::wait_for_rebirth，运行期）。
//      Agent 侧的 arm/rearm/clear 方法均委托至此，避免逻辑重复。
// ============================================================================

use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};
use uuid::Uuid;

use super::HttpApiState;

/// 布防自动注册倒计时（仅当未布防且 timeout>0 时）。返回是否布防成功。
pub async fn arm_deadline_if_absent(api_state: &Arc<HttpApiState>, timeout_secs: u64) -> bool {
    if timeout_secs == 0 {
        return false;
    }
    let mut deadline = api_state.auto_register_deadline.write().await;
    if deadline.is_some() {
        return false;
    }
    *deadline = Some(Instant::now() + Duration::from_secs(timeout_secs));
    info!(
        "[auto-register] 已布防自动注册倒计时: {}s 后自动生成角色（可先通过面板手动注册）",
        timeout_secs
    );
    true
}

/// 重新布防（自动注册失败后重试用，无条件覆盖）
pub async fn rearm_deadline(api_state: &Arc<HttpApiState>, timeout_secs: u64) {
    if timeout_secs == 0 {
        return;
    }
    *api_state.auto_register_deadline.write().await =
        Some(Instant::now() + Duration::from_secs(timeout_secs));
}

/// 清除倒计时（注册/转世成功后）
pub async fn clear_deadline(api_state: &Arc<HttpApiState>) {
    *api_state.auto_register_deadline.write().await = None;
}

/// 剩余秒数（未布防时为 None）
pub async fn remaining_secs(api_state: &Arc<HttpApiState>) -> Option<u64> {
    api_state
        .auto_register_deadline
        .read()
        .await
        .map(|d| d.saturating_duration_since(Instant::now()).as_secs())
}

/// 超时自动生成并注册角色（loopback 调用自身 HTTP API）
///
/// 与运维脚本 restart.sh 的手动流程同构：generate（LLM）→ register（转发 server）。
/// 生成/注册各重试 3 次（flaky LLM 兜底），单次最长 180s。
pub async fn auto_register_via_loopback(api_state: &Arc<HttpApiState>) -> Result<Uuid, String> {
    let port = api_state.actual_port;
    if port == 0 {
        return Err("HTTP 端口未知（actual_port=0）".to_string());
    }
    let token = api_state
        .device_config
        .read()
        .await
        .as_ref()
        .map(|c| c.auth_token.clone())
        .ok_or_else(|| "设备身份未初始化".to_string())?;

    let base = format!("http://127.0.0.1:{}", port);
    let client = reqwest::Client::new();
    for attempt in 1..=3u32 {
        let gen_resp = client
            .post(format!("{}/api/v1/character/generate", base))
            .header("Authorization", format!("Bearer {}", token))
            .json(&serde_json::json!({}))
            .timeout(Duration::from_secs(180))
            .send()
            .await
            .map_err(|e| format!("generate 请求失败: {}", e))?;
        if !gen_resp.status().is_success() {
            let st = gen_resp.status();
            let body = gen_resp.text().await.unwrap_or_default();
            warn!(
                "[auto-register] 第 {}/3 次生成失败: {} {}",
                attempt, st, body
            );
            if attempt < 3 {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            continue;
        }
        let character: serde_json::Value = gen_resp
            .json()
            .await
            .map_err(|e| format!("generate 响应解析失败: {}", e))?;
        info!(
            "[auto-register] 自动生成角色: {}",
            character["name"].as_str().unwrap_or("?")
        );

        let reg = client
            .post(format!("{}/api/v1/character/register", base))
            .header("Authorization", format!("Bearer {}", token))
            .json(&character)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| format!("register 请求失败: {}", e))?;
        let status = reg.status();
        let body: serde_json::Value = reg.json().await.unwrap_or(serde_json::Value::Null);
        if status.is_success() && body["agent_id"].as_str().is_some_and(|s| !s.is_empty()) {
            let id = body["agent_id"]
                .as_str()
                .unwrap_or_default()
                .parse::<Uuid>()
                .map_err(|e| format!("agent_id 解析失败: {}", e))?;
            // 立即清除倒计时（面板停止显示）；重连信号由 register handler 广播
            clear_deadline(api_state).await;
            return Ok(id);
        }
        warn!(
            "[auto-register] 第 {}/3 次注册失败: {} {}",
            attempt, status, body
        );
        if attempt < 3 {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
    Err("3 次尝试均失败".to_string())
}
