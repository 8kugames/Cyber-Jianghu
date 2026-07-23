// ============================================================================
// GET /api/v1/agent/by-device — 设备关联的活跃 Agent 查询
//
// 用途：agent 端 WS 重连后从 server 拉取已注册角色，补全本地 character 状态。
// Phase 4 联调发现：server API register 不会触发 agent WS Registered 消息，
// agent 端 character_not_registered 状态卡死。本端点提供「reload」通道。
//
// 鉴权：device token（与其他 agent 端点一致）
// 语义：仅返回 status='active' 的 agent（不暴露 dead/retired 历史）
// ============================================================================

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::db::{self, verify_device_token};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct GetAgentByDeviceRequest {
    pub device_id: Uuid,
    pub auth_token: String,
}

#[derive(Debug, Serialize)]
pub struct GetAgentByDeviceResponse {
    pub agent_id: String,
    pub name: String,
    /// 角色年龄（来自 agents.birth_tick 反推失败时用 default 25）
    pub age: u8,
    /// 角色性别（default "男"）
    pub gender: String,
    pub system_prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// 最近一次 agent_states.attributes（HashMap 拍平版，用于恢复 birth_attributes）
    #[serde(default)]
    pub initial_attributes: HashMap<String, i32>,
}

/// POST /api/v1/agent/by-device
///
/// 设备持有者查询自己当前绑定的活跃 Agent。
/// 用于 agent 端 reload 已注册角色（不走 WS Registered 通道时）。
pub async fn get_agent_by_device(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<GetAgentByDeviceRequest>,
) -> Result<Json<GetAgentByDeviceResponse>, (StatusCode, Json<serde_json::Value>)> {
    // 1. 验证设备 token
    let valid = verify_device_token(&state.db_pool, payload.device_id, &payload.auth_token)
        .await
        .map_err(|e| {
            warn!("设备验证 DB 错误: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal_error", "message": e.to_string()})),
            )
        })?;

    if !valid {
        warn!("设备 {} token 验证失败", payload.device_id);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid_device_token"})),
        ));
    }

    // 2. 查询 device 关联的 active agent
    let agent = db::get_agent_by_device_id(&state.db_pool, payload.device_id)
        .await
        .map_err(|e| {
            warn!("查询 Agent by device 失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal_error", "message": e.to_string()})),
            )
        })?;

    let Some(agent) = agent else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no_agent_for_device",
                "message": "设备未关联任何角色"
            })),
        ));
    };

    if agent.status != "active" {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "agent_not_active",
                "message": format!("角色状态: {}（仅 active 可 reload）", agent.status)
            })),
        ));
    }

    // 3. 拉取最近一次 agent_states.attributes（用于补全 birth_attributes）
    let initial_attributes = match db::get_latest_agent_state(&state.db_pool, agent.agent_id).await
    {
        Ok(state) => state.get_attributes_for_protocol(),
        Err(e) => {
            warn!("查询初始属性失败（容错用空 HashMap）: {}", e);
            HashMap::new()
        }
    };

    info!(
        "设备 {} reload agent {} ({})",
        payload.device_id, agent.agent_id, agent.name
    );

    Ok(Json(GetAgentByDeviceResponse {
        agent_id: agent.agent_id.to_string(),
        name: agent.name,
        age: 25, // birth_tick 未反推，default 25（reload 不影响 logic）
        gender: "男".to_string(),
        system_prompt: agent.system_prompt,
        model_id: agent.model_id,
        initial_attributes,
    }))
}