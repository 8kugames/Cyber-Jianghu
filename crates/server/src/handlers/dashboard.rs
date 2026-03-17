use axum::{
    extract::{Path, State},
    Json,
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
}

#[derive(Serialize)]
pub struct WorldTime {
    pub year: i32,
    pub month: i32,
    pub day: i32,
    pub hour: i32,
    pub minute: i32,
    pub second: i32,
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

    // 9. Game time
    let current_world_tick_id = crate::db::get_current_world_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    // Calculate game time (1 tick = 1 game hour)
    // 1 year = 12 months = 360 days = 8640 hours
    // 1 month = 30 days = 720 hours
    // 1 day = 24 hours
    let total_hours = current_world_tick_id;
    let year = 1 + (total_hours / 8640) as i32;
    let remaining_after_year = total_hours % 8640;
    let month = 1 + (remaining_after_year / 720) as i32;
    let remaining_after_month = remaining_after_year % 720;
    let day = 1 + (remaining_after_month / 24) as i32;
    let hour = (remaining_after_month % 24) as i32;
    // Minute and second are always 0 at tick boundaries
    let minute = 0;
    let second = 0;

    let game_time = WorldTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    };

    let game_flow_total_hours = total_hours;

    // 10. World overview
    // Try to load world building rules
    let world_overview = crate::websocket::types::load_world_building_rules()
        .map(|rules| rules.narrative_rules)
        .unwrap_or_else(|| "世界概览暂不可用".to_string());

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
        tick_duration_secs: state.config.tick_engine.tick_duration_secs,
    })
}

// ============================================================================
// Agent API
// ============================================================================

#[derive(Serialize)]
pub struct OnlineAgent {
    pub id: Uuid,
    pub name: String,
    pub location: String,
    pub hp: i32,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
}

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

    // 从数据库查询存活的 Agent（从 JSONB attributes 中提取 hp）
    let query = "
        SELECT
            a.agent_id,
            a.name,
            COALESCE(s.node_id, 'unknown') as location,
            COALESCE((s.attributes->>'hp')::int, 100) as hp,
            a.last_tick_online
        FROM agents a
        LEFT JOIN agent_states s ON a.agent_id = s.agent_id AND s.tick_id = $1
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

    tracing::debug!("返回存活且在线的 agent 数量: {}", agents.len());
    Json(agents)
}

#[derive(Serialize)]
pub struct OfflineAgent {
    pub id: Uuid,
    pub name: String,
    pub location: String,
    pub hp: i32,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
    pub is_alive: bool,
}

pub async fn get_offline_agents(State(state): State<Arc<AppState>>) -> Json<Vec<OfflineAgent>> {
    let connected_agents: std::collections::HashSet<Uuid> = {
        let connections = state.connection_manager.read().await;
        connections.keys().copied().collect()
    };

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
            COALESCE(s.is_alive, false) as is_alive,
            a.last_tick_online
        FROM agents a
        LEFT JOIN LatestStates s ON a.agent_id = s.agent_id
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
            is_alive: row.get("is_alive"),
        })
        .filter(|a| !connected_agents.contains(&a.id))
        .collect();

    tracing::debug!("返回离线/历史 agent 数量: {}", agents.len());
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
        (
            row.get::<String, _>("node_id"),
            row.get::<i32, _>("hp"),
            row.get::<i32, _>("hunger"),
            row.get::<i32, _>("thirst"),
            row.get::<i32, _>("stamina"),
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
