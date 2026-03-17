use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use std::sync::Arc;
use tracing::{error, info};

use crate::game_data;
use crate::models::{self, AgentRegisterRequest, AgentRegisterResponse};
use crate::state::AppState;

/// Agent降生注册接口
///
/// POST /api/v1/agent/register
///
/// 实现Agent注册流程（事务性）：
/// 1. 验证 name 和 system_prompt
/// 2. 在单个事务中执行：
///    - 创建Agent记录
///    - 创建初始状态（使用当前 tick_id）
///    - 分配默认初始物品
/// 3. 构建并返回游戏规则
///
/// 注意：根据架构原则，服务器只负责世界状态和规则。
/// Agent人设Prompt应由客户端（Agent SDK）提供，服务器仅存储。
pub async fn agent_register(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AgentRegisterRequest>,
) -> Result<Json<AgentRegisterResponse>, StatusCode> {
    info!("Agent registration request: {}", payload.name);

    // 1. 验证 name 长度
    if payload.name.is_empty() || payload.name.len() > models::get_max_agent_name_length() {
        error!("Invalid name length: {} chars", payload.name.len());
        return Err(StatusCode::BAD_REQUEST);
    }

    // 2. 获取并验证 system_prompt（必需）
    let system_prompt = payload.system_prompt.ok_or_else(|| {
        error!("Missing required field: system_prompt");
        StatusCode::BAD_REQUEST
    })?;

    // 验证 system_prompt 长度，防止滥用
    if system_prompt.is_empty() || system_prompt.len() > models::get_max_system_prompt_length() {
        error!(
            "Invalid system_prompt length: {} bytes",
            system_prompt.len()
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    // 3. 获取当前服务器的 tick_id（避免新 agent 累积"出生前"伤害）
    let current_tick_id = crate::db::get_current_world_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    // 4. 准备初始物品数据
    let initial_items = game_data::InitialInventoryRegistry::items();
    let initial_items_data: Vec<(String, String, i32, String)> = initial_items
        .iter()
        .map(|item| (item.item_id.clone(), item.name.clone(), item.quantity, item.description.clone()))
        .collect();

    // 5. 事务性注册（F-04：原子性保证）
    let registration = match crate::db::register_agent_transactional(
        &state.db_pool,
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
    info!("Agent '{}' registered successfully (transactional)!", agent.name);

    // 6. 构建游戏规则（从配置动态获取）
    let game_rules = crate::models::GameRules {
        tick_duration_secs: state.config.tick_engine.tick_duration_secs,
        available_actions: game_data::ActionRegistry::all_action_names()
            .into_iter()
            .map(|action_name| {
                let description = game_data::ActionRegistry::get(&action_name)
                    .and_then(|config| Some(config.description))
                    .unwrap_or_else(|| "".to_string());
                crate::models::AvailableAction {
                    action: action_name,
                    description,
                    valid_targets: None,
                }
            })
            .collect(),
        initial_items: initial_items
            .into_iter()
            .map(|item| crate::models::InitialItem {
                item_id: item.item_id,
                name: item.name,
                quantity: item.quantity,
                description: item.description,
            })
            .collect(),
        version: state.game_data.get().game_rules.version.clone(),
        last_updated: chrono::Utc::now().to_rfc3339(),
    };

    // 7. 获取叙事化配置（用于属性描述转换）
    let narrative_config = state.game_data.get().narrative.clone();

    Ok(Json(AgentRegisterResponse {
        agent_id: agent.agent_id.to_string(),
        auth_token: agent.auth_token,
        message: format!("Agent '{}' registered successfully", agent.name),
        game_rules,
        narrative_config,
    }))
}
