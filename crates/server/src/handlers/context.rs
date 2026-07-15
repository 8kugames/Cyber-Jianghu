// ============================================================================
// Agent Context Handler
// ============================================================================
//
// Provides a narrative-formatted agent context for OpenClaw integration
//
// GET /api/v1/agent/{id}/context
// ============================================================================

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;
use tracing::{debug, info};
use uuid::Uuid;

use crate::game_data;
use crate::game_data::registry::ItemRegistry;
use crate::game_data::types::StatusComponentExt;
use crate::state::AppState;

/// Agent context response
///
/// Narrative-formatted agent state for LLM consumption
#[derive(Debug, Clone, Serialize)]
pub struct AgentContextResponse {
    /// Agent ID
    pub agent_id: String,
    /// Agent name
    pub agent_name: String,
    /// Current location (node_id)
    pub location: String,
    /// Current tick ID
    pub tick_id: i64,
    /// Game time description
    pub game_time: String,
    /// Self status (narrative description)
    pub self_status: String,
    /// Inventory summary
    pub inventory: String,
    /// Nearby entities (names and descriptions)
    pub nearby_entities: String,
    /// Nearby items (names and descriptions)
    pub nearby_items: String,
    /// Available actions
    pub available_actions: Vec<String>,
    /// Status effects (active)
    pub status_effects: Vec<String>,
    /// Whether the agent is alive
    pub is_alive: bool,
}

/// Get agent context
///
/// Returns a narrative-formatted agent state for OpenClaw integration
///
/// # Parameters
/// - `id`: Agent UUID
///
/// # Returns
/// - `AgentContextResponse`: Narrative-formatted agent state
pub async fn get_agent_context(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<AgentContextResponse>, StatusCode> {
    let agent_id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;

    debug!("Getting context for agent: {}", agent_id);

    // Get current tick ID
    let current_tick_id = match crate::db::get_current_world_tick_id(&state.db_pool).await {
        Ok(tick_id) => tick_id,
        Err(e) => {
            tracing::error!("Failed to get current tick ID: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Get all alive agents to find the current agent
    let all_agents = match crate::db::get_all_alive_agents_latest_states(&state.db_pool).await {
        Ok(agents) => agents,
        Err(e) => {
            tracing::error!("Failed to get agents: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Find the current agent's state
    let agent_state = match all_agents.iter().find(|a| a.agent_id == agent_id) {
        Some(state) => state.clone(),
        None => {
            return Err(StatusCode::NOT_FOUND);
        }
    };

    // Get agent name from WebSocket connections (or use a default for now)
    // In a real implementation, you'd need to query the agent table or maintain a mapping
    let agent_name = format!("Agent-{}", agent_id.to_string().split_at(8).0);

    // 查询真实库存：从 agent_inventory 取 (item_id, quantity)，再用 ItemRegistry 解析名称
    // C4 修复：原占位符 "(查看物品详情需要额外查询)" 已替换为真实数据。
    let inventory_summary = build_inventory_summary(&state.db_pool, agent_id).await;

    // Get ground items at this location
    let scene_items =
        match crate::db::get_ground_items_by_node(&state.db_pool, &agent_state.node_id).await {
            Ok(items) => items,
            Err(e) => {
                tracing::error!("Failed to get scene items: {}", e);
                Vec::new()
            }
        };

    // For scene items, show item_id and quantity
    let nearby_items: Vec<String> = scene_items
        .iter()
        .map(|item| format!("{} x{}", item.item_id, item.quantity))
        .collect();

    // Generate narrative self-status
    let hp = agent_state.status.get_attr_value("hp").unwrap_or(100);
    let satiation = agent_state
        .status
        .get_attr_value("satiation")
        .unwrap_or(100);
    let hydration = agent_state
        .status
        .get_attr_value("hydration")
        .unwrap_or(100);
    let stamina = agent_state.status.get_attr_value("stamina").unwrap_or(100);

    let hp_status = if hp > 80 {
        "身体状况极佳，精力充沛"
    } else if hp > 50 {
        "身体状况一般，有些疲惫"
    } else if hp > 20 {
        "身体虚弱，伤痛明显"
    } else {
        "生命垂危"
    };

    let satiation_status = if satiation > 80 {
        "肚子很饱"
    } else if satiation > 40 {
        "肚子有些饿了"
    } else {
        "饥肠辘辘"
    };

    let hydration_status = if hydration > 80 {
        "完全不渴"
    } else if hydration > 40 {
        "有些口渴"
    } else {
        "非常口渴"
    };

    let stamina_status = if stamina > 80 {
        "体力充沛"
    } else if stamina > 40 {
        "体力有些不支"
    } else {
        "精疲力竭"
    };

    let self_status = format!(
        "### 自身状态\n- 身体: {}\n- 饱食度: {}\n- 饱饮度: {}\n- 体力: {}\n",
        hp_status, satiation_status, hydration_status, stamina_status
    );

    // Build nearby entities (filter by same location)
    let nearby_entities: Vec<String> = all_agents
        .iter()
        .filter(|a| a.agent_id != agent_id && a.node_id == agent_state.node_id)
        .map(|a| {
            let id_str = format!("Agent-{}", a.agent_id.to_string().split_at(8).0);
            if a.is_alive {
                format!("{} (存活)", id_str)
            } else {
                format!("{} (已死亡)", id_str)
            }
        })
        .collect();

    let nearby_entities_str = if nearby_entities.is_empty() {
        "附近没有其他人".to_string()
    } else {
        nearby_entities.join("、")
    };

    // nearby_items was already built above
    let nearby_items_str = if nearby_items.is_empty() {
        "地面没有任何物品".to_string()
    } else {
        format!("地上有: {}", nearby_items.join("、"))
    };

    // Get available actions
    let available_actions = game_data::ActionRegistry::all_action_names();

    // Get game time
    let game_time = format!("Tick {}", current_tick_id);

    info!("Context generated for agent: {} ({})", agent_name, agent_id);

    Ok(Json(AgentContextResponse {
        agent_id: agent_id.to_string(),
        agent_name: agent_name.clone(),
        location: agent_state.node_id.clone(),
        tick_id: current_tick_id,
        game_time,
        self_status,
        inventory: inventory_summary,
        nearby_entities: nearby_entities_str,
        nearby_items: nearby_items_str,
        available_actions,
        status_effects: Vec::new(),
        is_alive: agent_state.is_alive,
    }))
}

/// 构建叙事化背包摘要（C4 修复占位符）
///
/// 从 agent_inventory 表取 (item_id, quantity)，再用 ItemRegistry 把 item_id
/// 解析成可读的物品名。查库失败或背包为空时返回友好占位，永不阻塞主流程。
async fn build_inventory_summary(db_pool: &crate::db::DbPool, agent_id: Uuid) -> String {
    let rows = match sqlx::query(
        "SELECT item_id, quantity FROM agent_inventory WHERE agent_id = $1 AND quantity > 0 ORDER BY quantity DESC",
    )
    .bind(agent_id)
    .fetch_all(db_pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("查询 agent_inventory 失败 (agent={}): {}", agent_id, e);
            return "背包状态未知".to_string();
        }
    };

    if rows.is_empty() {
        return "背包空空如也".to_string();
    }

    let items: Vec<String> = rows
        .into_iter()
        .map(|row| {
            let item_id: String = row.get("item_id");
            let quantity: i32 = row.get("quantity");
            // 优先用 ItemRegistry 解析显示名，回退到 item_id
            let display_name = ItemRegistry::get(&item_id)
                .map(|cfg| cfg.name)
                .unwrap_or(item_id);
            format!("{} x{}", display_name, quantity)
        })
        .collect();

    format!("背包: {}", items.join("、"))
}
