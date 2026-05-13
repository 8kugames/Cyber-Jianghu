use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::AppState;
use cyber_jianghu_protocol::types::world::{number_to_chinese, shichen_name};

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

    // Bug #5: 新增监控指标
    pub natural_deaths_last_24h: i64,
    pub abnormal_deaths_last_24h: i64,
    pub offline_duration_distribution: OfflineDistribution,
}

#[derive(Serialize)]
pub struct OfflineDistribution {
    pub less_than_1h: i64,
    pub one_to_24h: i64,
    pub one_to_7d: i64,
    pub more_than_7d: i64,
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
    /// 天道历格式文本
    pub text: String,
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
        connections.values().map(|c| c.agent_id).collect()
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
    let season =
        crate::game_data::registry::TimeRegistry::get_current_season(current_world_tick_id)
            .map(|s| s.name)
            .unwrap_or_else(|| "未知".to_string());

    let month_text = match month {
        1 => "元月",
        2 => "二月",
        3 => "三月",
        4 => "四月",
        5 => "五月",
        6 => "六月",
        7 => "七月",
        8 => "八月",
        9 => "九月",
        10 => "十月",
        11 => "十一月",
        12 => "腊月",
        _ => unreachable!(),
    };
    let text = format!(
        "天道历{}年{}{}日{}",
        number_to_chinese(year),
        month_text,
        number_to_chinese(day),
        shichen_name(hour)
    );

    let game_time = WorldTime {
        year,
        month,
        day,
        hour,
        minute: 0, // 前端会自行计算平滑值
        second: 0, // 前端会自行计算平滑值
        season,
        text,
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

    // 11. Offline distribution
    let mut less_than_1h = 0;
    let mut one_to_24h = 0;
    let mut one_to_7d = 0;
    let mut more_than_7d = 0;

    let offline_rows = sqlx::query(
        "SELECT EXTRACT(EPOCH FROM (NOW() - last_tick_online))::FLOAT8 as offline_secs
         FROM agents WHERE last_tick_online IS NOT NULL",
    )
    .fetch_all(&state.db_pool)
    .await
    .unwrap_or_default();

    for row in offline_rows {
        let secs: Option<f64> = row.get(0);
        if let Some(s) = secs {
            if s < 3600.0 {
                less_than_1h += 1;
            } else if s < 86400.0 {
                one_to_24h += 1;
            } else if s < 604800.0 {
                one_to_7d += 1;
            } else {
                more_than_7d += 1;
            }
        }
    }

    // 12. Death statistics (simplified implementation using recent state changes)
    let dead_agents = sqlx::query(
        "SELECT COUNT(*) FROM agent_states s1
         JOIN agent_states s2 ON s1.agent_id = s2.agent_id AND s1.tick_id = s2.tick_id - 1
         WHERE s1.is_alive = true AND s2.is_alive = false",
    )
    .fetch_one(&state.db_pool)
    .await
    .map(|r| r.get::<i64, _>(0))
    .unwrap_or(0);

    // 假设目前所有死亡都是自然死亡（饿死等），这里作简化处理。
    // 如果有异常导致的死亡，可以在后续按需分类。
    let natural_deaths_last_24h = dead_agents;
    let abnormal_deaths_last_24h = 0;

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
        natural_deaths_last_24h,
        abnormal_deaths_last_24h,
        offline_duration_distribution: OfflineDistribution {
            less_than_1h,
            one_to_24h,
            one_to_7d,
            more_than_7d,
        },
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
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
}

/// 在线 Agent：返回最新 tick 中在线且存活的 agent
pub async fn get_online_agents(State(state): State<Arc<AppState>>) -> Json<Vec<OnlineAgent>> {
    let connected_agents: std::collections::HashSet<Uuid> = {
        let connections = state.connection_manager.read().await;
        connections.values().map(|c| c.agent_id).collect()
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
            a.created_at,
            a.last_tick_online
        FROM agents a
        INNER JOIN agent_states s ON a.agent_id = s.agent_id AND s.tick_id = $1
        WHERE s.is_alive = true
        ORDER BY a.created_at DESC
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
            created_at: row.get("created_at"),
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
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
}

/// 离线 Agent：返回存活但不在线的 agent
pub async fn get_offline_agents(State(state): State<Arc<AppState>>) -> Json<Vec<OfflineAgent>> {
    let connected_agents: std::collections::HashSet<Uuid> = {
        let connections = state.connection_manager.read().await;
        connections.values().map(|c| c.agent_id).collect()
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
            a.created_at,
            a.last_tick_online
        FROM agents a
        INNER JOIN LatestStates s ON a.agent_id = s.agent_id
        WHERE s.is_alive = true
        ORDER BY a.created_at DESC
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
            created_at: row.get("created_at"),
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
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
    pub is_alive: bool,
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
            a.created_at,
            a.last_tick_online,
            s.is_alive
        FROM agents a
        INNER JOIN LatestStates s ON a.agent_id = s.agent_id
        WHERE s.is_alive = false
        ORDER BY a.created_at DESC
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
            created_at: row.get("created_at"),
            last_active: row.get("last_tick_online"),
            is_alive: row.get("is_alive"),
        })
        .collect();

    tracing::debug!("返回已死亡的 agent 数量: {}", agents.len());
    Json(agents)
}

// ============================================================================
// 统一 Agent 列表（数据驱动状态）
// ============================================================================

/// Agent 列表条目（统一格式）
#[derive(Serialize)]
pub struct AgentListEntry {
    pub id: Uuid,
    pub name: String,
    pub device_id: Uuid,
    pub status: String,
    pub location: String,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
    pub last_tick_id: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub hp: i32,
    pub max_hp: i32,
    pub attributes: std::collections::HashMap<String, i32>,
    pub birth_attributes: std::collections::HashMap<String, i32>,
}

/// 获取所有 agents（统一列表，数据驱动）
pub async fn get_all_agents(State(state): State<Arc<AppState>>) -> Json<Vec<AgentListEntry>> {
    let connected_agents: std::collections::HashSet<Uuid> = {
        let connections = state.connection_manager.read().await;
        connections.values().map(|c| c.agent_id).collect()
    };

    // 从配置获取主属性名集合（数据驱动）
    let primary_attr_keys: std::collections::HashSet<String> = {
        let gd = state.game_data.get();
        gd.attributes
            .data
            .primary
            .attributes
            .keys()
            .cloned()
            .collect()
    };

    // 查询所有 agents 的最新状态
    let query = "
        WITH LatestStates AS (
            SELECT DISTINCT ON (agent_id) agent_id, node_id, attributes, is_alive, tick_id
            FROM agent_states
            ORDER BY agent_id, tick_id DESC
        )
        SELECT
            a.agent_id,
            a.name,
            a.device_id,
            a.status as db_status,
            a.created_at,
            a.last_tick_online,
            COALESCE(s.node_id, 'unknown') as location,
            COALESCE((s.attributes->>'hp')::int, 0) as hp,
            COALESCE((s.attributes->>'hp_max')::int, 100) as max_hp,
            s.is_alive,
            s.tick_id as last_tick_id,
            s.attributes as all_attrs
        FROM agents a
        LEFT JOIN LatestStates s ON a.agent_id = s.agent_id
        ORDER BY a.created_at DESC
        LIMIT 1000;
    ";

    let rows = sqlx::query(query)
        .fetch_all(&state.db_pool)
        .await
        .unwrap_or_default();

    let mut agents = Vec::new();

    for row in rows {
        let agent_id: Uuid = row.get("agent_id");
        let db_status: String = row.get("db_status");
        let is_alive: Option<bool> = row.get("is_alive");
        let _is_alive = is_alive.unwrap_or(false);

        // 确定状态（数据驱动）
        // 数据库状态优先：retired 和 dead 直接使用
        let status = if db_status == "retired" || db_status == "dead" {
            db_status
        } else if connected_agents.contains(&agent_id) {
            "online".to_string()
        } else {
            "offline".to_string()
        };

        // 从 JSONB all_attrs 拆分先天属性和状态属性（数据驱动）
        // 规则：key 或 key.trim_end_matches("_max") 在 primary_attr_keys 中 → birth_attributes
        let all_attrs: Option<serde_json::Value> = row.get("all_attrs");
        let mut attributes = std::collections::HashMap::new();
        let mut birth_attributes = std::collections::HashMap::new();

        if let Some(attrs) = all_attrs.and_then(|v| v.as_object().cloned()) {
            for (k, v) in &attrs {
                if let Some(val) = v.as_i64() {
                    let base_key = k.strip_suffix("_max").unwrap_or(k);
                    if primary_attr_keys.contains(base_key) {
                        birth_attributes.insert(k.clone(), val as i32);
                    } else {
                        attributes.insert(k.clone(), val as i32);
                    }
                }
            }
        }

        agents.push(AgentListEntry {
            id: agent_id,
            name: row.get("name"),
            device_id: row.get("device_id"),
            status,
            location: row.get("location"),
            last_active: row.get("last_tick_online"),
            last_tick_id: row.get("last_tick_id"),
            created_at: row.get("created_at"),
            hp: row.get("hp"),
            max_hp: row.get("max_hp"),
            attributes,
            birth_attributes,
        });
    }

    tracing::debug!("返回所有 agents 数量: {}", agents.len());
    Json(agents)
}

// ============================================================================
// 状态配置 API（数据驱动）
// ============================================================================

/// 状态配置项
#[derive(Serialize)]
pub struct StatusConfig {
    pub key: String,
    pub display_name: String,
    pub description: String,
    pub color: String,
    pub sort_order: i32,
}
/// 获取状态配置列表
pub async fn get_status_configs(State(state): State<Arc<AppState>>) -> Json<Vec<StatusConfig>> {
    let gd = state.game_data.get();
    let configs: Vec<StatusConfig> = gd
        .game_rules
        .data
        .agent_statuses
        .iter()
        .map(|(key, cfg)| StatusConfig {
            key: key.clone(),
            display_name: cfg.display_name.clone(),
            description: cfg.description.clone(),
            color: cfg.color.clone(),
            sort_order: cfg.sort_order,
        })
        .collect();
    Json(configs)
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
    pub max_hp: i32,
    pub hunger: i32,
    pub max_hunger: i32,
    pub thirst: i32,
    pub max_thirst: i32,
    pub stamina: i32,
    pub max_stamina: i32,
    pub is_alive: bool,
    pub inventory: Vec<AgentInventoryItem>,
    pub attributes: std::collections::HashMap<String, i32>,
    /// 当前年龄（游戏年），NULL = 不朽
    pub age: Option<i64>,
    /// 寿元上限（游戏年），NULL = 无上限
    pub max_age: Option<i64>,
    /// 纪传体传记（死亡/归隐时生成）
    pub biography: Option<String>,
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

    let (
        location,
        hp,
        max_hp,
        hunger,
        max_hunger,
        thirst,
        max_thirst,
        stamina,
        max_stamina,
        is_alive,
        mut attributes_map,
    ) = if let Some(ref row) = state_row {
        // 从 JSONB attributes 列提取属性值
        let attrs: serde_json::Value = row.get::<serde_json::Value, _>("attributes");

        let mut attributes_map = std::collections::HashMap::new();
        if let Some(obj) = attrs.as_object() {
            for (k, v) in obj {
                if let Some(val) = v.as_i64() {
                    attributes_map.insert(k.clone(), val as i32);
                }
            }
        }

        // 获取配置计算动态最大值
        let config = crate::game_data::registry::StateRegistry::get_attributes_config();
        let get_max = |name: &str| -> i32 {
            if let Some(cfg) = &config
                && let Some(attr_def) = cfg.data.status.attributes.get(name)
            {
                return crate::game_data::types::StatusComponent::evaluate_max_value(
                    &attr_def.max_value_formula,
                    100.0,
                    &attributes_map,
                ) as i32;
            }
            100
        };

        (
            row.get::<String, _>("node_id"),
            attrs.get("hp").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs
                .get("hp_max")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .unwrap_or_else(|| get_max("hp")),
            attrs.get("hunger").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs
                .get("hunger_max")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .unwrap_or_else(|| get_max("hunger")),
            attrs.get("thirst").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs
                .get("thirst_max")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .unwrap_or_else(|| get_max("thirst")),
            attrs.get("stamina").and_then(|v| v.as_i64()).unwrap_or(100) as i32,
            attrs
                .get("stamina_max")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .unwrap_or_else(|| get_max("stamina")),
            row.get::<bool, _>("is_alive"),
            attributes_map,
        )
    } else {
        (
            "unknown".to_string(),
            100,
            100,
            100,
            100,
            100,
            100,
            100,
            100,
            true,
            std::collections::HashMap::new(),
        )
    };

    // Calculate derived attributes
    if let Some(cfg) = crate::game_data::registry::StateRegistry::get_attributes_config() {
        let mut base_attrs = std::collections::HashMap::new();
        for (k, v) in &cfg.data.derived.attributes {
            base_attrs.insert(
                k.clone(),
                cyber_jianghu_protocol::AttributeMetadata {
                    name: v.name.clone(),
                    display_name: v.display_name.clone(),
                    description: v.description.clone(),
                    formula: v.formula.clone(),
                    affects: vec![],
                    attr_type: cyber_jianghu_protocol::AttributeType::Derived,
                    birth_range: None,
                    default_value: None,
                    min_value: None,
                    max_value_formula: None,
                    decay_per_tick: None,
                    death_condition: None,
                    initial_value: None,
                    growth_rate: None,
                    recovery_formula: None,
                    primary_attribute_deps: vec![],
                },
            );
        }
        let derived_component =
            crate::game_data::types::components::DerivedAttributeComponent::from_config(
                &base_attrs,
            );
        let formula_engine = crate::game_data::formula_engine::FormulaEngine::new();

        let mut context_i64 = std::collections::HashMap::new();
        for (k, v) in &attributes_map {
            context_i64.insert(k.clone(), *v as f64);
        }

        for name in cfg.data.derived.attributes.keys() {
            if let Ok(val) = derived_component.calculate(name, &formula_engine, &context_i64) {
                attributes_map.insert(name.clone(), val as i32);
            }
        }
    }

    // 计算年龄与寿元
    let age = if let Some(birth_tick) = agent_row.get::<Option<i64>, _>("birth_tick") {
        let current_tick = state_row
            .as_ref()
            .map(|r| r.get::<i64, _>("tick_id"))
            .unwrap_or(0);
        if birth_tick > 0 && birth_tick < current_tick {
            Some(crate::tick::decay::compute_age_years(
                birth_tick,
                current_tick,
            ))
        } else {
            Some(0)
        }
    } else {
        None
    };
    let max_age = state
        .game_data
        .get_lifespan_config()
        .map(|(m, _, _)| m as i64);

    Ok(Json(AgentDetail {
        id: agent_row.get("agent_id"),
        name: agent_row.get("name"),
        system_prompt: agent_row.get("system_prompt"),
        created_at: agent_row.get("created_at"),
        last_active: agent_row.get("last_tick_online"),
        location,
        hp,
        max_hp,
        hunger,
        max_hunger,
        thirst,
        max_thirst,
        stamina,
        max_stamina,
        is_alive,
        inventory,
        attributes: attributes_map,
        age,
        max_age,
        biography: agent_row.get("biography"),
    }))
}

// ============================================================================
// Maintenance API
// ============================================================================

#[derive(Serialize)]
pub struct CleanupResult {
    pub deleted_count: u64,
}

/// 清理长期离线的 Agent
pub async fn cleanup_offline_agents(
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    let cleanup_days = {
        let gd_guard = state.game_data.get();
        gd_guard.game_rules.data.ops.offline_cleanup_days
    };

    let mut tx = match state.db_pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("Failed to begin transaction for cleanup: {}", e);
            return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let query_str = format!(
        "DELETE FROM agents WHERE last_tick_online < NOW() - INTERVAL '{} days'",
        cleanup_days
    );

    let result = match sqlx::query(&query_str).execute(&mut *tx).await {
        Ok(res) => res,
        Err(e) => {
            tracing::error!("Failed to execute cleanup query: {}", e);
            return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if let Err(e) = tx.commit().await {
        tracing::error!("Failed to commit transaction for cleanup: {}", e);
        return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    tracing::info!(
        "Dashboard triggered cleanup: deleted {} agents.",
        result.rows_affected()
    );

    Ok(Json(CleanupResult {
        deleted_count: result.rows_affected(),
    }))
}

// ============================================================================
// Agent Experiences API
// ============================================================================

/// 经历日志条目
#[derive(Debug, serde::Serialize)]
pub struct ExperienceEntry {
    pub tick_id: i64,
    /// 动作原始类型（如 idle, speak）
    pub action_type: String,
    /// 动作中文描述（如 "休息，不做任何操作"）
    pub action_type_display: Option<String>,
    pub action_data: serde_json::Value,
    /// 执行结果（success/failed）
    pub result: Option<String>,
    /// 执行结果详细描述
    pub result_message: Option<String>,
    /// ActorSoul 思考日志
    pub thought_log: Option<String>,
    /// ReflectorSoul 审查理由
    pub observer_thought: Option<String>,
    /// 叙事化经历描述
    pub narrative: Option<String>,
    /// 三魂循环元数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soul_cycle_metadata: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 经历日志响应
#[derive(Debug, serde::Serialize)]
pub struct ExperiencesResponse {
    pub experiences: Vec<ExperienceEntry>,
    pub total: i64,
    pub page: i32,
    pub limit: i32,
}

/// 获取 Agent 经历日志
///
/// 支持两种认证方式：
/// 1. Admin token (Bearer auth): 查看任意角色的经历日志
/// 2. Device auth (query params): 设备只能查看自己归属角色的经历日志
///
/// GET /api/dashboard/agent/{id}/experiences?page=1&limit=20&device_id=xxx&auth_token=yyy
pub async fn get_agent_experiences(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ExperiencesResponse>, StatusCode> {
    // 设备认证：如果提供了 device_id 和 auth_token，使用设备归属校验
    if let (Some(device_id_str), Some(auth_token)) =
        (params.get("device_id"), params.get("auth_token"))
        && let Ok(device_id) = Uuid::parse_str(device_id_str)
    {
        match crate::db::verify_device_token(&state.db_pool, device_id, auth_token).await {
            Ok(true) => {
                // 验证通过，检查设备是否归属该 agent
                let owner_device_id: Option<Uuid> =
                    sqlx::query_scalar("SELECT device_id FROM agents WHERE agent_id = $1")
                        .bind(agent_id)
                        .fetch_optional(&state.db_pool)
                        .await
                        .unwrap_or(None);

                if owner_device_id != Some(device_id) {
                    tracing::warn!(
                        "Device {} attempted to access agent {} experiences without ownership",
                        device_id,
                        agent_id
                    );
                    return Err(StatusCode::FORBIDDEN);
                }
            }
            Ok(false) => return Err(StatusCode::UNAUTHORIZED),
            Err(e) => {
                tracing::warn!("Device token verify error: {}", e);
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    let page: i32 = params.get("page").and_then(|s| s.parse().ok()).unwrap_or(1);
    let limit: i32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let offset = (page - 1) * limit;

    // 获取总数
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_action_logs WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&state.db_pool)
            .await
            .unwrap_or(0);

    // 获取经历日志
    let rows = sqlx::query(
        "SELECT tick_id, action_type, action_type_display, action_data, result, result_message, thought_log, observer_thought, narrative, soul_cycle_metadata, created_at
         FROM agent_action_logs
         WHERE agent_id = $1
         ORDER BY tick_id DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(agent_id)
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch experiences: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let experiences: Vec<ExperienceEntry> = rows
        .into_iter()
        .map(|row| ExperienceEntry {
            tick_id: row.get("tick_id"),
            action_type: row.get("action_type"),
            action_type_display: row.get("action_type_display"),
            action_data: row
                .get::<Option<serde_json::Value>, _>("action_data")
                .unwrap_or(serde_json::Value::Null),
            result: row.get("result"),
            result_message: row.get("result_message"),
            thought_log: row.get("thought_log"),
            observer_thought: row.get("observer_thought"),
            narrative: row.get("narrative"),
            soul_cycle_metadata: row.get("soul_cycle_metadata"),
            created_at: row.get("created_at"),
        })
        .collect();

    Ok(Json(ExperiencesResponse {
        experiences,
        total,
        page,
        limit,
    }))
}

/// GET /api/dashboard/actions-map - 返回 action_type -> 中文名映射
///
/// 无需认证（action 映射不是敏感数据，供前端渲染使用）
pub async fn get_actions_map() -> Json<std::collections::HashMap<String, String>> {
    let map: std::collections::HashMap<String, String> =
        crate::game_data::ActionRegistry::build_available_actions()
            .into_iter()
            .map(|a| (a.action, a.name))
            .collect();
    Json(map)
}

// ============================================================================
// Experience Stream API (经历日志流水)
// ============================================================================

/// 经历日志流水查询参数
#[derive(Debug, Deserialize)]
pub struct ExperienceStreamQuery {
    pub page: Option<i32>,
    pub limit: Option<i32>,
    pub agent_id: Option<Uuid>,
    pub location: Option<String>,
    pub action_type: Option<String>,
    pub from_tick: Option<i64>,
    pub to_tick: Option<i64>,
    /// 结果过滤: "success" | "failed" | 空=全部
    pub result: Option<String>,
}

/// 经历日志流水条目
#[derive(Debug, Serialize)]
pub struct StreamEntry {
    pub tick_id: i64,
    pub agent_id: Uuid,
    pub device_id: Option<Uuid>,
    pub agent_name: String,
    pub location: Option<String>,
    pub action_type: String,
    pub action_type_display: Option<String>,
    pub action_data: serde_json::Value,
    pub result: Option<String>,
    pub result_message: Option<String>,
    pub thought_log: Option<String>,
    pub observer_thought: Option<String>,
    pub narrative: Option<String>,
    pub soul_cycle_metadata: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 经历日志流水响应
#[derive(Debug, Serialize)]
pub struct ExperienceStreamResponse {
    pub entries: Vec<StreamEntry>,
    pub total: i64,
    pub page: i32,
    pub limit: i32,
}

/// GET /api/dashboard/experiences
///
/// 返回 agent 动作日志（全局视图），用于经历日志流水。
/// 默认只返回成功记录，传 result=all 查看全部。
pub async fn get_experiences(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExperienceStreamQuery>,
) -> Result<Json<ExperienceStreamResponse>, StatusCode> {
    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * limit;

    // 构建过滤条件
    let agent_id_filter = params.agent_id;
    let location_filter = params.location;
    let action_type_filter = params.action_type;
    let from_tick_filter = params.from_tick;
    let to_tick_filter = params.to_tick;
    // result 过滤: None/空 → 只看成功, "failed" → 只看失败, "all" → 全部
    let result_filter = params.result.as_deref().unwrap_or("success");

    // 查询总数
    let total: i64 = sqlx::query_scalar(
        r#"
        WITH action_with_location AS (
            SELECT a.tick_id, a.agent_id,
                   loc.node_id as location
            FROM agent_action_logs a
            LEFT JOIN LATERAL (
                SELECT st2.node_id
                FROM agent_states st2
                WHERE st2.agent_id = a.agent_id AND st2.tick_id <= a.tick_id
                ORDER BY st2.tick_id DESC
                LIMIT 1
            ) loc ON true
            WHERE ($6::text = 'all' OR a.result = $6)
              AND ($1::uuid IS NULL OR a.agent_id = $1)
              AND ($3::text IS NULL OR a.action_type = $3)
              AND ($4::bigint IS NULL OR a.tick_id >= $4)
              AND ($5::bigint IS NULL OR a.tick_id <= $5)
        )
        SELECT COUNT(*)
        FROM action_with_location
        WHERE ($2::text IS NULL OR location = $2)
        "#,
    )
    .bind(agent_id_filter)
    .bind(&location_filter)
    .bind(&action_type_filter)
    .bind(from_tick_filter)
    .bind(to_tick_filter)
    .bind(result_filter)
    .fetch_one(&state.db_pool)
    .await
    .unwrap_or(0);

    // 查询条目：使用 LATERAL JOIN 获取动作发生时的位置
    let rows = sqlx::query(
        r#"
        SELECT a.tick_id, a.agent_id, ag.device_id, ag.name as agent_name, loc.node_id as location,
               a.action_type, a.action_type_display, a.action_data,
               a.result, a.result_message, a.thought_log, a.observer_thought,
               a.narrative, a.soul_cycle_metadata, a.created_at
        FROM agent_action_logs a
        JOIN agents ag ON a.agent_id = ag.agent_id
        LEFT JOIN LATERAL (
            SELECT st2.node_id
            FROM agent_states st2
            WHERE st2.agent_id = a.agent_id AND st2.tick_id <= a.tick_id
            ORDER BY st2.tick_id DESC
            LIMIT 1
        ) loc ON true
        WHERE ($6::text = 'all' OR a.result = $6)
          AND ($1::uuid IS NULL OR a.agent_id = $1)
          AND ($2::text IS NULL OR loc.node_id = $2)
          AND ($3::text IS NULL OR a.action_type = $3)
          AND ($4::bigint IS NULL OR a.tick_id >= $4)
          AND ($5::bigint IS NULL OR a.tick_id <= $5)
        ORDER BY a.tick_id DESC
        LIMIT $7 OFFSET $8
        "#,
    )
    .bind(agent_id_filter)
    .bind(&location_filter)
    .bind(&action_type_filter)
    .bind(from_tick_filter)
    .bind(to_tick_filter)
    .bind(limit as i64)
    .bind(offset as i64)
    .fetch_all(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("获取经历日志流水失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let entries: Vec<StreamEntry> = rows
        .into_iter()
        .map(|row| StreamEntry {
            tick_id: row.get("tick_id"),
            agent_id: row.get("agent_id"),
            device_id: row.get("device_id"),
            agent_name: row.get("agent_name"),
            location: row.get("location"),
            action_type: row.get("action_type"),
            action_type_display: row.get("action_type_display"),
            action_data: row
                .get::<Option<serde_json::Value>, _>("action_data")
                .unwrap_or(serde_json::Value::Null),
            result: row.get("result"),
            result_message: row.get("result_message"),
            thought_log: row.get("thought_log"),
            observer_thought: row.get("observer_thought"),
            narrative: row.get("narrative"),
            soul_cycle_metadata: row.get("soul_cycle_metadata"),
            created_at: row.get("created_at"),
        })
        .collect();

    Ok(Json(ExperienceStreamResponse {
        entries,
        total,
        page,
        limit,
    }))
}

// ============================================================================
// Items API (物品列表，供 Admin 面板 grant-items UI 使用)
// ============================================================================

/// 物品摘要（用于下拉选择器）
#[derive(Debug, Serialize)]
pub struct ItemSummary {
    pub item_id: String,
    pub name: String,
    pub item_type: String,
    pub description: String,
}

/// 获取所有已配置物品列表
///
/// GET /api/dashboard/items
pub async fn get_items() -> Json<Vec<ItemSummary>> {
    let items = crate::game_data::registry::ItemRegistry::all_item_ids()
        .iter()
        .filter_map(|id| crate::game_data::registry::ItemRegistry::get(id))
        .map(|entry| ItemSummary {
            item_id: entry.item_id,
            name: entry.name,
            item_type: entry.item_type,
            description: entry.description,
        })
        .collect();
    Json(items)
}
