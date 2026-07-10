use axum::{Json, extract::State};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::AppState;

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
    pub is_alive: bool,
    pub location: String,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
    pub last_tick_id: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub hp: i32,
    pub max_hp: i32,
    pub attributes: std::collections::HashMap<String, i32>,
    pub birth_attributes: std::collections::HashMap<String, i32>,
    pub roles: Vec<String>,
    /// 角色注册时上报的 LLM 模型 ID（如 glm-4、gpt-4o）
    pub model_id: Option<String>,
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
            a.model_id,
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

    let agent_ids: Vec<Uuid> = rows.iter().map(|r| r.get::<Uuid, _>("agent_id")).collect();

    let role_rows = if agent_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT agent_id, role_key FROM agent_assigned_roles WHERE agent_id = ANY($1)",
        )
        .bind(&agent_ids)
        .fetch_all(&state.db_pool)
        .await
        .unwrap_or_default()
    };

    let mut roles_map: std::collections::HashMap<Uuid, Vec<String>> =
        std::collections::HashMap::new();
    for (aid, rk) in &role_rows {
        roles_map.entry(*aid).or_default().push(rk.clone());
    }

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
            is_alive: _is_alive,
            location: row.get("location"),
            last_active: row.get("last_tick_online"),
            last_tick_id: row.get("last_tick_id"),
            created_at: row.get("created_at"),
            hp: row.get("hp"),
            max_hp: row.get("max_hp"),
            attributes,
            birth_attributes,
            roles: roles_map.remove(&agent_id).unwrap_or_default(),
            model_id: row.get("model_id"),
        });
    }

    tracing::debug!("返回所有 agents 数量: {}", agents.len());
    Json(agents)
}
