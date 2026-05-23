use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use super::super::Agent;

struct RebirthParams {
    old_agent_id: Uuid,
    delay_ms: u64,
    http_url: String,
    api_state: Arc<crate::infra::api::HttpApiState>,
    device_id: Uuid,
    auth_token: String,
    name: String,
    system_prompt: String,
    retry_max: u32,
    retry_interval: std::time::Duration,
    context: String,
}

fn schedule_auto_rebirth(params: RebirthParams) {
    let RebirthParams {
        old_agent_id,
        delay_ms,
        http_url,
        api_state,
        device_id,
        auth_token,
        name,
        system_prompt,
        retry_max,
        retry_interval,
        context,
    } = params;
    let context_label = context.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        info!(
            "自动转世重生{}: 调用 auto-rebirth API (old_agent={})",
            context_label, old_agent_id
        );

        let client = reqwest::Client::new();
        let url = format!("{}/api/v1/agent/auto-rebirth", http_url);
        let body = serde_json::json!({
            "device_id": device_id,
            "auth_token": auth_token,
            "old_agent_id": old_agent_id,
            "name": name,
            "system_prompt": system_prompt,
        });

        for attempt in 0..retry_max {
            match client.post(&url).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let data: serde_json::Value = resp.json().await.unwrap_or_default();
                    let new_id = data["new_agent_id"]
                        .as_str()
                        .and_then(|s| s.parse::<Uuid>().ok())
                        .unwrap_or(Uuid::nil());

                    info!(
                        "自动转世重生成功: old_agent={} → new_agent={}",
                        old_agent_id, new_id
                    );

                    *api_state.pending_rebirth_agent_id.write().await = Some(new_id);
                    api_state
                        .is_dead
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    api_state.rebirth_notify.notify_waiters();
                    return;
                }
                Ok(resp) => {
                    let status = resp.status();
                    let resp_body = resp.text().await.unwrap_or_default();
                    warn!(
                        "自动转世重生服务端拒绝 (attempt {}/{}): status={}, body={}",
                        attempt + 1,
                        retry_max,
                        status,
                        resp_body
                    );
                }
                Err(e) => {
                    warn!(
                        "自动转世重生网络错误 (attempt {}/{}): {}",
                        attempt + 1,
                        retry_max,
                        e
                    );
                }
            }
            if attempt + 1 < retry_max {
                tokio::time::sleep(retry_interval).await;
            }
        }
        tracing::error!(
            "自动转世重生最终失败{}: old_agent={}, 所有 {} 次重试用尽",
            context_label,
            old_agent_id,
            retry_max
        );
    });
}

pub(super) async fn maybe_schedule_auto_rebirth(
    agent: &Agent,
    _old_agent_id: Uuid,
    world_state: &cyber_jianghu_protocol::WorldState,
    context: &str,
) {
    let auto_rebirth_enabled = agent
        .http_api_state
        .as_ref()
        .map(|s| s.auto_rebirth.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(true);

    if agent.rebirth_delay_ticks <= 0 || !auto_rebirth_enabled {
        return;
    }

    let delay_ticks = agent.rebirth_delay_ticks;
    let tick_secs = agent.get_tick_duration().await.as_secs();
    let delay_ms = delay_ticks as u64 * tick_secs * 1000;

    let old_agent_id = world_state
        .agent_id
        .or_else(|| agent.character_config.as_ref().and_then(|c| c.agent_id))
        .unwrap_or_default();

    let http_url = agent.config.server.http_url.clone();
    let Some(api_state) = agent.http_api_state.clone() else {
        return;
    };

    let Some(device_cfg) = agent.device_config.as_ref() else {
        warn!("自动转世重生跳过: device_config 未设置");
        return;
    };
    let device_id = device_cfg.device_id;
    let auth_token = device_cfg.auth_token.clone();

    let (name, system_prompt) = agent
        .character_config
        .as_ref()
        .map(|cc| {
            (
                cc.name.clone(),
                cc.system_prompt.clone().unwrap_or_default(),
            )
        })
        .unwrap_or_default();

    let retry_max = agent
        .config
        .game_rules
        .as_ref()
        .map(|r| r.rebirth_retry_max_attempts)
        .unwrap_or(3);
    let retry_interval = std::time::Duration::from_secs(
        agent
            .config
            .game_rules
            .as_ref()
            .map(|r| r.rebirth_retry_interval_secs)
            .unwrap_or(30),
    );

    if old_agent_id == Uuid::nil() {
        warn!(
            "自动转世重生跳过: 无法获取有效的 old_agent_id \
             (world_state.agent_id=None, api_state.agent_id=None)"
        );
        return;
    }

    info!(
        "自动转世重生已调度{}: agent={}, delay={} ticks ({}s)",
        context,
        old_agent_id,
        delay_ticks,
        delay_ms / 1000
    );

    schedule_auto_rebirth(RebirthParams {
        old_agent_id,
        delay_ms,
        http_url,
        api_state,
        device_id,
        auth_token,
        name,
        system_prompt,
        retry_max,
        retry_interval,
        context: context.to_string(),
    });
}
