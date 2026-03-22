use anyhow::Result;
use axum::{Json, extract::State, http::StatusCode};
use std::sync::Arc;
use tracing::{error, info};

use crate::db::{self, verify_device_token, DeviceConnectResult};
use crate::game_data;
use crate::models::{
    get_max_agent_name_length, get_max_system_prompt_length, AgentConnectRequest,
    AgentConnectResponse, AgentRegisterRequest, AgentRegisterResponse, AvailableAction,
    GameRules, InitialItem,
};
use crate::state::AppState;

// ============================================================================
// 设备连接 API（Phase 3）
// ============================================================================

/// 设备连接接口
///
/// POST /api/v1/agent/connect
///
/// 客户端首次启动时调用，用于注册设备身份或获取现有认证令牌。
///
/// 流程：
/// 1. 客户端生成 device_id (UUID v4)
/// 2. 调用此接口注册设备
/// 3. 服务器返回 auth_token
/// 4. 客户端保存 device_id + auth_token 到 agent.yaml
///
/// 后续 WebSocket 连接使用: ws://server/ws?device_id={}&token={}
pub async fn agent_connect(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AgentConnectRequest>,
) -> Result<Json<AgentConnectResponse>, StatusCode> {
    info!("设备连接请求: {}", payload.device_id);

    // 注册或获取设备
    let DeviceConnectResult {
        device_id,
        auth_token,
        is_new,
    } = db::connect_device(&state.db_pool, payload.device_id)
        .await
        .map_err(|e| {
            error!("设备连接失败: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let message = if is_new {
        format!("设备 {} 注册成功", device_id)
    } else {
        format!("设备 {} 已连接", device_id)
    };

    info!("{}", message);

    Ok(Json(AgentConnectResponse {
        auth_token,
        message,
    }))
}

// ============================================================================
// Agent 注册 API（Phase 4 - 角色创建）
// ============================================================================

/// Agent降生注册接口
///
/// POST /api/v1/agent/register
///
/// 实现Agent注册流程（事务性）：
/// 1. 验证设备认证（device_id + auth_token）
/// 2. 验证 name 和 system_prompt
/// 3. 在单个事务中执行：
///    - 创建Agent记录
///    - 创建初始状态（使用当前 tick_id）
///    - 分配默认初始物品
/// 4. 构建并返回游戏规则
///
/// 注意：根据架构原则，服务器只负责世界状态和规则。
/// Agent人设Prompt应由客户端（Agent SDK）提供，服务器仅存储。
pub async fn agent_register(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AgentRegisterRequest>,
) -> Result<Json<AgentRegisterResponse>, StatusCode> {
    info!("Agent registration request: {}", payload.name);

    // 1. 验证设备认证（device_id + auth_token）
    let device_valid = verify_device_token(&state.db_pool, payload.device_id, &payload.auth_token)
        .await
        .map_err(|e| {
            error!("Device verification failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !device_valid {
        error!(
            "Invalid device credentials: device_id={}",
            payload.device_id
        );
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 2. 验证 name 长度
    if payload.name.is_empty() || payload.name.len() > get_max_agent_name_length() {
        error!("Invalid name length: {} chars", payload.name.len());
        return Err(StatusCode::BAD_REQUEST);
    }

    // 3. 获取并验证 system_prompt（必需）
    let system_prompt = payload.system_prompt.ok_or_else(|| {
        error!("Missing required field: system_prompt");
        StatusCode::BAD_REQUEST
    })?;

    // 验证 system_prompt 长度，防止滥用
    if system_prompt.is_empty() || system_prompt.len() > get_max_system_prompt_length() {
        error!(
            "Invalid system_prompt length: {} bytes",
            system_prompt.len()
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    // 4. 获取当前服务器的 tick_id（避免新 agent 累积"出生前"伤害）
    let current_tick_id = crate::db::get_current_world_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    // 5. 准备初始物品数据
    let initial_items = game_data::InitialInventoryRegistry::items();
    let initial_items_data: Vec<(String, String, i32, String)> = initial_items
        .iter()
        .map(|item| {
            (
                item.item_id.clone(),
                item.name.clone(),
                item.quantity,
                item.description.clone(),
            )
        })
        .collect();

    // 6. 事务性注册（F-04：原子性保证）
    let registration = match crate::db::register_agent_transactional(
        &state.db_pool,
        payload.device_id, // 关联设备ID
        &payload.name,
        &system_prompt,
        current_tick_id,
        &initial_items_data,
    )
    .await
    {
        Ok(reg) => reg,
        Err(e) => {
            error!("Agent registration transaction failed: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let agent = registration.agent;
    info!(
        "Agent '{}' registered successfully (transactional)!",
        agent.name
    );

    // 7. 构建游戏规则（从配置动态获取）
    let tick_duration_secs = {
        let gd = state.game_data.get();
        gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64
    };
    let game_rules = GameRules {
        tick_duration_secs,
        available_actions: game_data::ActionRegistry::all_action_names()
            .into_iter()
            .map(|action_name| {
                let description = game_data::ActionRegistry::get(&action_name).map(|config| config.description)
                    .unwrap_or_default();
                AvailableAction {
                    action: action_name,
                    description,
                    valid_targets: None,
                }
            })
            .collect(),
        initial_items: initial_items
            .into_iter()
            .map(|item| InitialItem {
                item_id: item.item_id,
                name: item.name,
                quantity: item.quantity,
                description: item.description,
            })
            .collect(),
        version: state.game_data.get().game_rules.version.clone(),
        last_updated: chrono::Utc::now().to_rfc3339(),
    };

    // 8. 获取叙事化配置（用于属性描述转换）
    let narrative_config = state.game_data.get().narrative.clone();

    // 9. 获取初始属性（先天属性，用于 Agent 端存储 birth_attributes）
    let initial_attributes = registration.initial_state.get_attributes_for_protocol();

    Ok(Json(AgentRegisterResponse {
        agent_id: agent.agent_id.to_string(),
        message: format!("Agent '{}' registered successfully", agent.name),
        game_rules,
        narrative_config,
        initial_attributes,
    }))
}

// ============================================================================
// Agent 转生 API（Phase 4 - 归隐重生）
// ============================================================================

/// 转生请求
#[derive(Debug, serde::Deserialize)]
pub struct RebirthRequest {
    /// 设备 ID
    pub device_id: uuid::Uuid,
    /// 认证令牌
    pub auth_token: String,
}

/// 转生响应
#[derive(Debug, serde::Serialize)]
pub struct RebirthResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
    /// 归隐的角色 ID
    pub retired_agent_id: Option<String>,
}

/// Agent 转生接口
///
/// POST /api/v1/agent/rebirth
///
/// 删除当前设备的角色，保留设备身份，允许重新创建新角色。
/// 由于 agents 表有 ON DELETE CASCADE，会自动删除：
/// - agent_states 表中的所有状态记录
/// - agent_inventory 表中的所有物品记录
pub async fn agent_rebirth(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RebirthRequest>,
) -> Result<Json<RebirthResponse>, StatusCode> {
    info!("Agent 转生请求: device_id={}", payload.device_id);

    // 调用数据库操作
    match db::rebirth_agent(&state.db_pool, payload.device_id, &payload.auth_token).await {
        Ok(result) => {
            info!(
                "Agent 转生成功: {} ({}) 已归隐",
                result.retired_name,
                result.retired_agent_id
            );
            Ok(Json(RebirthResponse {
                success: true,
                message: format!("角色 '{}' 已归隐，可以创建新角色", result.retired_name),
                retired_agent_id: Some(result.retired_agent_id.to_string()),
            }))
        }
        Err(e) => {
            error!("Agent 转生失败: {}", e);
            Err(StatusCode::BAD_REQUEST)
        }
    }
}
