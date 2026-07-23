// ============================================================================
// POST /api/v1/admin/reload-character — 从 server reload 已注册角色
//
// 用途：解决 Phase 4 联调发现的集成 gap——
//
//   server API register (POST /api/v1/agent/register) 写 DB 后不触发
//   agent 端 WS Registered 消息，agent 端 character 状态卡死。
//
//   本端点从 server 拉已注册角色 → 写 character.yaml → 更新运行时 agent_id
//   → 触发 WS reconnect → cognitive loop 自动启动。
//
// 调用链：agent 容器内 curl POST localhost:23340/api/v1/admin/reload-character
// ============================================================================

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::{CharacterConfig, CharacterStatus, LanguageStyleConfig, GoalsConfig};
use crate::infra::api::ReconnectRequest;

use super::HttpApiState;
use super::character_helpers::{get_device_id, save_character};

#[derive(Debug, Serialize)]
pub struct ReloadCharacterResponse {
    pub agent_id: String,
    pub name: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ServerByDeviceResponse {
    agent_id: String,
    name: String,
    age: u8,
    gender: String,
    system_prompt: String,
    /// 服务端记录的 LLM 模型 ID（reload 暂不写入本地，仅透传以备未来使用）
    #[serde(default)]
    #[allow(dead_code)]
    model_id: Option<String>,
    #[serde(default)]
    initial_attributes: HashMap<String, i32>,
}

/// POST /api/v1/admin/reload-character
///
/// 从 server 拉设备关联的 active agent，写本地 character.yaml，
/// 更新运行时 agent_id，触发 WS reconnect。
pub async fn reload_character(
    State(state): State<HttpApiState>,
) -> Result<Json<ReloadCharacterResponse>, (StatusCode, Json<ReloadCharacterResponse>)> {
    // 1. 读 device.yaml 拿 device_id + auth_token
    let (device_id, auth_token) = match get_device_id(&state).await {
        Ok(id) => id,
        Err(e) => {
            error!("reload-character: 无设备身份: {}", e);
            return Err((
                StatusCode::PRECONDITION_FAILED,
                Json(ReloadCharacterResponse {
                    agent_id: String::new(),
                    name: String::new(),
                    message: "设备身份未初始化，请先启动 Agent".to_string(),
                    warning: None,
                }),
            ));
        }
    };

    // 2. 调 server /api/v1/agent/by-device
    let server_http_url = state.server_http_url.read().await.clone();
    let server_url = format!("{}/api/v1/agent/by-device", server_http_url);

    let client = reqwest::Client::new();
    let req_body = serde_json::json!({
        "device_id": device_id,
        "auth_token": auth_token,
    });

    let response = match client
        .post(&server_url)
        .json(&req_body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            error!("reload-character: 连接 server 失败: {}", e);
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(ReloadCharacterResponse {
                    agent_id: String::new(),
                    name: String::new(),
                    message: format!("连接 server 失败: {}", e),
                    warning: None,
                }),
            ));
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        error!("reload-character: server 拒绝 {} - {}", status, body);
        return Err((
            status,
            Json(ReloadCharacterResponse {
                agent_id: String::new(),
                name: String::new(),
                message: format!("server 拒绝: {}", body),
                warning: None,
            }),
        ));
    }

    let server_resp: ServerByDeviceResponse = match response.json().await {
        Ok(r) => r,
        Err(e) => {
            error!("reload-character: 解析 server 响应失败: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ReloadCharacterResponse {
                    agent_id: String::new(),
                    name: String::new(),
                    message: format!("解析响应失败: {}", e),
                    warning: None,
                }),
            ));
        }
    };

    let agent_uuid = match Uuid::parse_str(&server_resp.agent_id) {
        Ok(id) => id,
        Err(e) => {
            error!("reload-character: 解析 agent_id 失败: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ReloadCharacterResponse {
                    agent_id: server_resp.agent_id,
                    name: server_resp.name,
                    message: format!("解析 agent_id 失败: {}", e),
                    warning: None,
                }),
            ));
        }
    };

    info!(
        "reload-character: 从 server 拉到 agent {} ({})",
        agent_uuid, server_resp.name
    );

    // 3. 构造 CharacterConfig（reload 场景不重写 appearance/identity/personality/values——
    //    server 端 register handler 已经把这些拼到 system_prompt 里，重复字段无意义）
    let character = CharacterConfig {
        agent_id: Some(agent_uuid),
        name: server_resp.name.clone(),
        age: server_resp.age,
        gender: server_resp.gender,
        appearance: None,
        identity: None,
        personality: Vec::new(),
        values: Vec::new(),
        language_style: LanguageStyleConfig::default(),
        goals: GoalsConfig::default(),
        system_prompt: Some(server_resp.system_prompt.clone()),
        registered_at: Some(chrono::Utc::now()),
        birth_attributes: if server_resp.initial_attributes.is_empty() {
            None
        } else {
            Some(server_resp.initial_attributes.clone())
        },
        status: CharacterStatus::Alive,
        server_url: Some(server_http_url.clone()),
        last_connected_real_time: None,
        last_connected_world_time: None,
        biography: None,
    };

    // 4. 写 character.yaml
    let character_dir = state.character_dir.read().await.clone();
    let mut warning = None;
    if let Err(e) = save_character(&character, &character_dir) {
        error!("reload-character: 保存 character.yaml 失败: {}", e);
        warning = Some(format!("保存 character.yaml 失败: {}", e));
    }

    // 5. 更新运行时 agent_id（使后续 Intent 提交使用新 agent_id）
    {
        let mut id = state.agent_id.write().await;
        *id = agent_uuid;
        info!(
            "[reload-character] 更新运行时 agent_id 为 {} ({})",
            agent_uuid, server_resp.name
        );
    }

    // 6. 触发 WS reconnect（让 cognitive loop 重新连 server 并启动）
    if let Some(ref tx) = state.reconnect_tx {
        let server_ws_url = state.server_ws_url.read().await.clone();
        let reconnect_req = ReconnectRequest {
            ws_url: server_ws_url,
            agent_id: Some(agent_uuid),
        };
        if let Err(e) = tx.send(reconnect_req) {
            warn!("reload-character: 触发重连失败: {}", e);
        } else {
            info!("reload-character: 已触发 WS 重连");
        }
    }

    // 7. 重置死亡状态
    state
        .is_dead
        .store(false, std::sync::atomic::Ordering::Relaxed);

    Ok(Json(ReloadCharacterResponse {
        agent_id: agent_uuid.to_string(),
        name: server_resp.name,
        message: "从 server reload 成功，WS 重连已触发".to_string(),
        warning,
    }))
}