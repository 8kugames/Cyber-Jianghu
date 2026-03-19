use axum::{
    Json,
    extract::{Path, State},
};
use chrono::Utc;
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::AppState;

#[derive(Serialize)]
pub struct DashboardStats {
    pub current_active_agents: i64,
    pub total_registered_agents: i64,
    pub dau: i64,
    pub active_3d: i64,
    pub active_7d: i64,
    pub mau: i64,
    pub yau: i64,
    pub server_uptime_secs: i64,
    pub server_running_days: i64,
    pub game_time: WorldTime,
    pub game_flow_total_hours: i64,
    pub world_overview: String,
    pub tick_duration_secs: u64,
    /// 当前 tick ID（供前端计算平滑时间）
    pub current_tick_id: i64,
    /// 每游戏小时对应的 tick 数（供前端计算平滑时间）
    pub ticks_per_hour: f64,
}

#[derive(Serialize)]
pub struct WorldTime {
    pub year: i32,
    pub month: i32,
    pub day: i32,
    pub hour: i32,
    pub minute: i32,
    pub second: i32,
    /// 当前季节名称
    pub season: String,
}

pub async fn get_dashboard_stats(State(state): State<Arc<AppState>>) -> Json<DashboardStats> {
    // 1. Total registered agents
    let total_registered_agents: i64 = sqlx::query("SELECT COUNT(*) FROM agents")
        .fetch_one(&state.db_pool)
        .await
        .map(|r| r.get(0))
        .unwrap_or(0);

    // 2. Current active agents (alive and in latest tick and online)
    let connected_agents: std::collections::HashSet<Uuid> = {
        let connections = state.connection_manager.read().await;
        connections.keys().copied().collect()
    };
    let latest_state_tick_id = crate::db::get_latest_state_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    let alive_agents: Vec<Uuid> = sqlx::query(
        "SELECT agent_id FROM agent_states 
         WHERE is_alive = true 
         AND tick_id = $1",
    )
    .bind(latest_state_tick_id)
    .fetch_all(&state.db_pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| r.get(0))
    .collect();

    let current_active_agents = alive_agents
        .into_iter()
        .filter(|id| connected_agents.contains(id))
        .count() as i64;

    // 3. DAU (Active in last 24h)
    // Active means: submitted an intent (last_tick_online) OR created in the last 24h
    let dau: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT agent_id) FROM agents 
         WHERE last_tick_online > NOW() - INTERVAL '1 day'
         OR created_at > NOW() - INTERVAL '1 day'",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get(0))
    .unwrap_or(0);

    // 4. 3-Day Active
    let active_3d: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT agent_id) FROM agents 
         WHERE last_tick_online > NOW() - INTERVAL '3 days'
         OR created_at > NOW() - INTERVAL '3 days'",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get(0))
    .unwrap_or(0);

    // 5. 7-Day Active
    let active_7d: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT agent_id) FROM agents 
         WHERE last_tick_online > NOW() - INTERVAL '7 days'
         OR created_at > NOW() - INTERVAL '7 days'",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get(0))
    .unwrap_or(0);

    // 6. MAU (30 days)
    let mau: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT agent_id) FROM agents 
         WHERE last_tick_online > NOW() - INTERVAL '30 days'
         OR created_at > NOW() - INTERVAL '30 days'",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get(0))
    .unwrap_or(0);

    // 7. YAU (1 year)
    let yau: i64 = sqlx::query(
        "SELECT COUNT(DISTINCT agent_id) FROM agents 
         WHERE last_tick_online > NOW() - INTERVAL '1 year'
         OR created_at > NOW() - INTERVAL '1 year'",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get(0))
    .unwrap_or(0);

    // 8. Server uptime
    let now = Utc::now();
    let uptime = now.signed_duration_since(state.start_time);
    let server_uptime_secs = uptime.num_seconds();
    let server_running_days = uptime.num_days();

    // 9. Game time - 基础数据供前端计算平滑时间
    let current_world_tick_id = crate::db::get_current_world_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    // 从配置读取时间参数
    let config = crate::game_data::registry::TimeRegistry::get_config();
    let ticks_per_hour = config
        .as_ref()
        .map(|c| c.ticks_per_hour as f64)
        .unwrap_or(1.0);
    let hours_per_day = config
        .as_ref()
        .map(|c| c.hours_per_day as i64)
        .unwrap_or(24);
    let days_per_month = 30;
    let months_per_year = 12;
    let hours_per_month = hours_per_day * days_per_month;
    let hours_per_year = hours_per_month * months_per_year;

    // 计算整数游戏时间（前端会自行计算平滑的小数部分）
    let total_game_hours = current_world_tick_id as i64 / ticks_per_hour as i64;

    let year = 1 + (total_game_hours / hours_per_year) as i32;
    let remaining_after_year = total_game_hours % hours_per_year;
    let month = 1 + (remaining_after_year / hours_per_month) as i32;
    let remaining_after_month = remaining_after_year % hours_per_month;
    let day = 1 + (remaining_after_month / hours_per_day) as i32;
    let hour = (remaining_after_month % hours_per_day) as i32;

    // 获取季节信息
    let season = crate::game_data::registry::TimeRegistry::get_current_season(current_world_tick_id)
        .map(|s| s.name)
        .unwrap_or_else(|| "未知".to_string());

    let game_time = WorldTime {
        year,
        month,
        day,
        hour,
        minute: 0,  // 前端会自行计算平滑值
        second: 0,  // 前端会自行计算平滑值
        season,
    };

    let game_flow_total_hours = total_game_hours;

    // 10. World overview
    // Try to load world building rules
    let world_overview = crate::websocket::types::load_world_building_rules()
        .map(|rules| rules.narrative_rules)
        .unwrap_or_else(|| "世界概览暂不可用".to_string());

    // Get tick duration from game_data
    let tick_duration_secs = {
        let gd = state.game_data.get();
        gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64
    };

    Json(DashboardStats {
        current_active_agents,
        total_registered_agents,
        dau,
        active_3d,
        active_7d,
        mau,
        yau,
        server_uptime_secs,
        server_running_days,
        game_time,
        game_flow_total_hours,
        world_overview,
        tick_duration_secs,
        current_tick_id: current_world_tick_id,
        ticks_per_hour,
    })
}

// ============================================================================
// Agent API
// ============================================================================
//
// 逻辑拆分：
// 1. 在线 agent：最新 tick 在线且存活
// 2. 离线 agent：存活但不在线
// 3. 已死亡 agent：已死亡的 agent 列表
//

/// 在线 Agent（最新 tick 在线且存活）
#[derive(Serialize)]
pub struct OnlineAgent {
    pub id: Uuid,
    pub name: String,
    pub location: String,
    pub hp: i32,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
}

/// 在线 Agent：返回最新 tick 中在线且存活的 agent
pub async fn get_online_agents(State(state): State<Arc<AppState>>) -> Json<Vec<OnlineAgent>> {
    let connected_agents: std::collections::HashSet<Uuid> = {
        let connections = state.connection_manager.read().await;
        connections.keys().copied().collect()
    };

    if connected_agents.is_empty() {
        return Json(vec![]);
    }

    let latest_state_tick_id = crate::db::get_latest_state_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    // 查询最新 tick 中存活的 Agent，然后过滤在线的
    let query = "
        SELECT
            a.agent_id,
            a.name,
            COALESCE(s.node_id, 'unknown') as location,
            COALESCE((s.attributes->>'hp')::int, 100) as hp,
            a.last_tick_online
        FROM agents a
        INNER JOIN agent_states s ON a.agent_id = s.agent_id AND s.tick_id = $1
        WHERE s.is_alive = true
        ORDER BY a.last_tick_online DESC NULLS LAST
    ";

    let rows = sqlx::query(query)
        .bind(latest_state_tick_id)
        .fetch_all(&state.db_pool)
        .await
        .unwrap_or_default();

    let agents: Vec<OnlineAgent> = rows
        .into_iter()
        .map(|row| OnlineAgent {
            id: row.get("agent_id"),
            name: row.get("name"),
            location: row.get("location"),
            hp: row.get("hp"),
            last_active: row.get("last_tick_online"),
        })
        .filter(|a| connected_agents.contains(&a.id))
        .collect();

    tracing::debug!("返回在线且存活的 agent 数量: {}", agents.len());
    Json(agents)
}

/// 离线 Agent（存活但不在线）
#[derive(Serialize)]
pub struct OfflineAgent {
    pub id: Uuid,
    pub name: String,
    pub location: String,
    pub hp: i32,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
}

/// 离线 Agent：返回存活但不在线的 agent
pub async fn get_offline_agents(State(state): State<Arc<AppState>>) -> Json<Vec<OfflineAgent>> {
    let connected_agents: std::collections::HashSet<Uuid> = {
        let connections = state.connection_manager.read().await;
        connections.keys().copied().collect()
    };

    // 查询最新状态中存活但不在线的 agent
    let query = "
        WITH LatestStates AS (
            SELECT DISTINCT ON (agent_id) agent_id, node_id, attributes, is_alive
            FROM agent_states
            ORDER BY agent_id, tick_id DESC
        )
        SELECT
            a.agent_id,
            a.name,
            COALESCE(s.node_id, 'unknown') as location,
            COALESCE((s.attributes->>'hp')::int, 100) as hp,
            a.last_tick_online
        FROM agents a
        INNER JOIN LatestStates s ON a.agent_id = s.agent_id
        WHERE s.is_alive = true
        ORDER BY a.last_tick_online DESC NULLS LAST
        LIMIT 200;
    ";

    let rows = sqlx::query(query)
        .fetch_all(&state.db_pool)
        .await
        .unwrap_or_default();

    let agents: Vec<OfflineAgent> = rows
        .into_iter()
        .map(|row| OfflineAgent {
            id: row.get("agent_id"),
            name: row.get("name"),
            location: row.get("location"),
            hp: row.get("hp"),
            last_active: row.get("last_tick_online"),
        })
        .filter(|a| !connected_agents.contains(&a.id))
        .collect();

    tracing::debug!("返回离线但存活的 agent 数量: {}", agents.len());
    Json(agents)
}

/// 已死亡 Agent
#[derive(Serialize)]
pub struct DeadAgent {
    pub id: Uuid,
    pub name: String,
    pub location: String,
    pub hp: i32,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
}

/// 已死亡 Agent：返回已死亡的 agent 列表
pub async fn get_dead_agents(State(state): State<Arc<AppState>>) -> Json<Vec<DeadAgent>> {
    // 查询最新状态中已死亡的 agent
    let query = "
        WITH LatestStates AS (
            SELECT DISTINCT ON (agent_id) agent_id, node_id, attributes, is_alive
            FROM agent_states
            ORDER BY agent_id, tick_id DESC
        )
        SELECT
            a.agent_id,
            a.name,
            COALESCE(s.node_id, 'unknown') as location,
            COALESCE((s.attributes->>'hp')::int, 0) as hp,
            a.last_tick_online
        FROM agents a
        INNER JOIN LatestStates s ON a.agent_id = s.agent_id
        WHERE s.is_alive = false
        ORDER BY a.last_tick_online DESC NULLS LAST
        LIMIT 200;
    ";

    let rows = sqlx::query(query)
        .fetch_all(&state.db_pool)
        .await
        .unwrap_or_default();

    let agents: Vec<DeadAgent> = rows
        .into_iter()
        .map(|row| DeadAgent {
            id: row.get("agent_id"),
            name: row.get("name"),
            location: row.get("location"),
            hp: row.get("hp"),
            last_active: row.get("last_tick_online"),
        })
        .collect();

    tracing::debug!("返回已死亡的 agent 数量: {}", agents.len());
    Json(agents)
}

#[derive(Serialize)]
pub struct AgentDetail {
    pub id: Uuid,
    pub name: String,
    pub system_prompt: String,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
    pub location: String,
    pub hp: i32,
    pub hunger: i32,
    pub thirst: i32,
    pub stamina: i32,
    pub is_alive: bool,
    pub inventory: Vec<AgentInventoryItem>,
}

#[derive(Serialize)]
pub struct AgentInventoryItem {
    pub item_id: String,
    pub name: String,
    pub count: i32,
    pub is_equipped: bool,
}

pub async fn get_agent_details(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<AgentDetail>, axum::http::StatusCode> {
    // 1. Get basic info
    let agent_row = sqlx::query("SELECT * FROM agents WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_optional(&state.db_pool)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let agent_row = match agent_row {
        Some(row) => row,
        None => return Err(axum::http::StatusCode::NOT_FOUND),
    };

    // 2. Get latest state
    let state_row =
        sqlx::query("SELECT * FROM agent_states WHERE agent_id = $1 ORDER BY tick_id DESC LIMIT 1")
            .bind(agent_id)
            .fetch_optional(&state.db_pool)
            .await
            .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    // 3. Get inventory
    let inventory_rows = sqlx::query(
        "SELECT ai.item_id, i.name, ai.quantity, ai.is_equipped 
         FROM agent_inventory ai
         JOIN items i ON ai.item_id = i.item_id
         WHERE ai.agent_id = $1",
    )
    .bind(agent_id)
    .fetch_all(&state.db_pool)
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let inventory = inventory_rows
        .into_iter()
        .map(|row| AgentInventoryItem {
            item_id: row.get("item_id"),
            name: row.get("name"),
            count: row.get("quantity"),
            is_equipped: row.get("is_equipped"),
        })
        .collect();

    let (location, hp, hunger, thirst, stamina, is_alive) = if let Some(row) = state_row {
        // 从 JSONB attributes 列提取属性值
        let attrs: serde_json::Value = row.get::<serde_json::Value, _>("attributes");
        (
            row.get::<String, _>("node_id"),
            attrs.get("hp").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs.get("hunger").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs.get("thirst").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs.get("stamina").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            row.get::<bool, _>("is_alive"),
        )
    } else {
        ("unknown".to_string(), 100, 100, 100, 100, true)
    };

    Ok(Json(AgentDetail {
        id: agent_row.get("agent_id"),
        name: agent_row.get("name"),
        system_prompt: agent_row.get("system_prompt"),
        created_at: agent_row.get("created_at"),
        last_active: agent_row.get("last_tick_online"),
        location,
        hp,
        hunger,
        thirst,
        stamina,
        is_alive,
        inventory,
    }))
}
