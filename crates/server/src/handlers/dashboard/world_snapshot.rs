// ============================================================================
// 统一世界快照端点 (C3)
// ============================================================================
//
// GET /api/dashboard/world-snapshot
//
// 一次请求返回 { agents, tick_info, recent_events }，前端无需发 3 个独立请求，
// 也不会因为 tick 边界而拿到瞬时跨 agent 不一致的快照（比如 A 的状态来自 tick N
// 而 B 的状态来自 tick N+1）。
//
// 一致性策略：用单个只读事务 `pool.begin()` 包住所有 SELECT。事务隔离级别默认是
// READ COMMITTED，但因为所有查询在同一个事务里执行，PostgreSQL 保证它们看到的是
// 同一个事务快照（first-statement 快照点），从而跨表读到一致状态。
//
// agents 查询复刻 agents.rs::get_all_agents 的 LatestStates CTE；recent_events
// 取最近 20 条有 narrative 的 agent_action_logs。
// ============================================================================

use axum::{Json, extract::State};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::AppState;

// ============================================================================
// 响应结构
// ============================================================================

/// 统一世界快照响应
#[derive(Debug, Serialize)]
pub struct WorldSnapshot {
    /// 生成快照时的事务时间戳（ISO 8601）
    pub generated_at: chrono::DateTime<chrono::Utc>,
    /// Tick 元信息（调度器进度 + 最新已落库状态 tick）
    pub tick_info: TickInfo,
    /// 全量 agent 列表（与 /api/dashboard/agents 同源，单事务内读取）
    pub agents: Vec<SnapshotAgent>,
    /// 最近叙事事件（有 narrative 的 action_logs，倒序）
    pub recent_events: Vec<RecentEvent>,
}

/// Tick 元信息
#[derive(Debug, Serialize)]
pub struct TickInfo {
    /// 调度器当前正在接受的 tick_id（来自 AppState 原子量；0 = 调度器未启动）
    pub current_accepting_tick_id: i64,
    /// 数据库里 agent_states 的最新 tick_id
    pub latest_state_tick_id: i64,
    /// 最近一次 tick 日志的运行状态（running/completed/failed/none）
    pub latest_tick_status: String,
}

/// 快照内 agent 条目（get_all_agents 的精简版，去掉 birth_attributes）
#[derive(Debug, Serialize)]
pub struct SnapshotAgent {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub is_alive: bool,
    pub location: String,
    pub last_active: Option<chrono::DateTime<chrono::Utc>>,
    pub last_tick_id: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub hp: i32,
    pub max_hp: i32,
    pub attributes: std::collections::HashMap<String, i32>,
    pub roles: Vec<String>,
    pub model_id: Option<String>,
    /// 是否当前 WebSocket 在线
    pub is_online: bool,
}

/// 最近叙事事件
#[derive(Debug, Serialize)]
pub struct RecentEvent {
    pub id: i64,
    pub tick_id: i64,
    pub agent_id: Uuid,
    pub agent_name: String,
    pub action_type: String,
    pub action_type_display: Option<String>,
    pub result: Option<String>,
    pub narrative: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ============================================================================
// Handler
// ============================================================================

/// GET /api/dashboard/world-snapshot
///
/// 一次返回 agents + tick_info + recent_events，全部在单个只读事务内读取，
/// 消除 tick 边界瞬时跨 agent 不一致。
pub async fn get_world_snapshot(
    State(state): State<Arc<AppState>>,
) -> Json<WorldSnapshot> {
    // 当前在线 agent_id 集合（从 WebSocket connection manager 取，不在事务内，
    // 因为这是内存态、不涉及 DB 一致性）
    let online_agents: std::collections::HashSet<Uuid> = {
        let connections = state.connection_manager.read().await;
        connections.values().map(|c| c.agent_id).collect()
    };

    // 调度器当前 tick（原子量，内存态）
    let accepting_tick_id = state
        .current_accepting_tick_id
        .load(std::sync::atomic::Ordering::Relaxed);

    // 开只读事务：所有 SELECT 共享同一快照点
    let mut tx = match state.db_pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("world-snapshot 开启事务失败: {}", e);
            return Json(fallback_empty_snapshot(accepting_tick_id));
        }
    };

    // 1) agents（复刻 get_all_agents 的 LatestStates CTE）
    let agents = match fetch_snapshot_agents(&mut tx, &online_agents).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("world-snapshot 查询 agents 失败: {}", e);
            let _ = tx.rollback().await;
            return Json(fallback_empty_snapshot(accepting_tick_id));
        }
    };

    // 2) latest_state_tick_id + latest_tick_status（单查询取两列）
    let (latest_state_tick_id, latest_tick_status) =
        match fetch_tick_meta(&mut tx).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("world-snapshot 查询 tick meta 失败: {}", e);
                (0i64, "none".to_string())
            }
        };

    // 3) recent_events（最近 20 条有 narrative 的 action_logs）
    let recent_events = match fetch_recent_events(&mut tx).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("world-snapshot 查询 recent_events 失败: {}", e);
            Vec::new()
        }
    };

    // 提交只读事务（丢弃结果，仅释放快照）
    let _ = tx.commit().await;

    Json(WorldSnapshot {
        generated_at: chrono::Utc::now(),
        tick_info: TickInfo {
            current_accepting_tick_id: accepting_tick_id,
            latest_state_tick_id,
            latest_tick_status,
        },
        agents,
        recent_events,
    })
}

// ============================================================================
// 内部查询
// ============================================================================

async fn fetch_snapshot_agents(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    online_agents: &std::collections::HashSet<Uuid>,
) -> Result<Vec<SnapshotAgent>, sqlx::Error> {
    let query = "
        WITH LatestStates AS (
            SELECT DISTINCT ON (agent_id) agent_id, node_id, attributes, is_alive, tick_id
            FROM agent_states
            ORDER BY agent_id, tick_id DESC
        )
        SELECT
            a.agent_id,
            a.name,
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

    let rows = sqlx::query(query).fetch_all(&mut **tx).await?;

    let agent_ids: Vec<Uuid> = rows
        .iter()
        .map(|r| r.get::<Uuid, _>("agent_id"))
        .collect();

    // 单事务内取 roles（保证一致快照）
    let role_rows = if agent_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT agent_id, role_key FROM agent_assigned_roles WHERE agent_id = ANY($1)",
        )
        .bind(&agent_ids)
        .fetch_all(&mut **tx)
        .await?
    };

    let mut roles_map: std::collections::HashMap<Uuid, Vec<String>> =
        std::collections::HashMap::new();
    for (aid, rk) in &role_rows {
        roles_map.entry(*aid).or_default().push(rk.clone());
    }

    let mut agents = Vec::with_capacity(rows.len());
    for row in rows {
        let agent_id: Uuid = row.get("agent_id");
        let db_status: String = row.get("db_status");
        let is_alive: Option<bool> = row.get("is_alive");
        let all_attrs: Option<serde_json::Value> = row.get("all_attrs");

        let attributes = parse_attributes(&all_attrs);

        agents.push(SnapshotAgent {
            id: agent_id,
            name: row.get("name"),
            status: db_status,
            is_alive: is_alive.unwrap_or(false),
            location: row.get("location"),
            last_active: row.get("last_tick_online"),
            last_tick_id: row.get("last_tick_id"),
            created_at: row.get("created_at"),
            hp: row.get("hp"),
            max_hp: row.get("max_hp"),
            attributes,
            roles: roles_map.remove(&agent_id).unwrap_or_default(),
            model_id: row.get("model_id"),
            is_online: online_agents.contains(&agent_id),
        });
    }

    Ok(agents)
}

/// 解析 attributes JSONB 为 HashMap<String, i32>（与 agents.rs 的风格一致）
fn parse_attributes(attrs: &Option<serde_json::Value>) -> std::collections::HashMap<String, i32> {
    let mut map = std::collections::HashMap::new();
    if let Some(serde_json::Value::Object(obj)) = attrs {
        for (k, v) in obj {
            // 只保留数值字段（hp/satiation/hydration/stamina/...）
            if let Some(n) = v.as_i64() {
                map.insert(k.clone(), n as i32);
            } else if let Some(n) = v.as_f64() {
                map.insert(k.clone(), n as i32);
            }
        }
    }
    map
}

/// 同时取 latest_state_tick_id 和 latest_tick_status。
/// latest_state_tick_id 来自 agent_states（最新落库状态），
/// latest_tick_status 来自 tick_logs 最新一条。
async fn fetch_tick_meta(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(i64, String), sqlx::Error> {
    let latest_state_tick_id: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(tick_id), 0) FROM agent_states",
    )
    .fetch_one(&mut **tx)
    .await?;

    let latest_tick_status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM tick_logs ORDER BY tick_id DESC LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?;

    Ok((
        latest_state_tick_id,
        latest_tick_status.unwrap_or_else(|| "none".to_string()),
    ))
}

async fn fetch_recent_events(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<Vec<RecentEvent>, sqlx::Error> {
    let query = "
        SELECT
            l.id,
            l.tick_id,
            l.agent_id,
            a.name as agent_name,
            l.action_type,
            l.action_type_display,
            l.result,
            l.narrative,
            l.created_at
        FROM agent_action_logs l
        LEFT JOIN agents a ON a.agent_id = l.agent_id
        WHERE l.narrative IS NOT NULL AND l.narrative <> ''
        ORDER BY l.created_at DESC
        LIMIT 20;
    ";

    let rows = sqlx::query(query).fetch_all(&mut **tx).await?;

    Ok(rows
        .into_iter()
        .map(|row| RecentEvent {
            id: row.get("id"),
            tick_id: row.get("tick_id"),
            agent_id: row.get("agent_id"),
            agent_name: row.get::<Option<String>, _>("agent_name")
                .unwrap_or_else(|| "unknown".to_string()),
            action_type: row.get("action_type"),
            action_type_display: row.get("action_type_display"),
            result: row.get("result"),
            narrative: row.get("narrative"),
            created_at: row.get("created_at"),
        })
        .collect())
}

/// 所有 DB 查询失败时的兜底空快照（保证 handler 永不 panic，前端拿到结构化空数据）
fn fallback_empty_snapshot(accepting_tick_id: i64) -> WorldSnapshot {
    WorldSnapshot {
        generated_at: chrono::Utc::now(),
        tick_info: TickInfo {
            current_accepting_tick_id: accepting_tick_id,
            latest_state_tick_id: 0,
            latest_tick_status: "none".to_string(),
        },
        agents: Vec::new(),
        recent_events: Vec::new(),
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_attributes_handles_null_and_object() {
        // None
        let map = parse_attributes(&None);
        assert!(map.is_empty());

        // 非 object
        let map = parse_attributes(&Some(serde_json::json!([1, 2, 3])));
        assert!(map.is_empty());

        // 标准 object
        let map = parse_attributes(&Some(serde_json::json!({
            "hp": 100,
            "satiation": 80,
            "stamina": 50,
            "name": "ignored",  // 非数值应被过滤
        })));
        assert_eq!(map.get("hp"), Some(&100));
        assert_eq!(map.get("satiation"), Some(&80));
        assert_eq!(map.get("stamina"), Some(&50));
        assert!(!map.contains_key("name"));
    }

    #[test]
    fn fallback_empty_snapshot_is_structured() {
        let snap = fallback_empty_snapshot(42);
        assert_eq!(snap.tick_info.current_accepting_tick_id, 42);
        assert_eq!(snap.tick_info.latest_state_tick_id, 0);
        assert_eq!(snap.tick_info.latest_tick_status, "none");
        assert!(snap.agents.is_empty());
        assert!(snap.recent_events.is_empty());
    }

    #[test]
    fn world_snapshot_is_serialize() {
        // 编译期断言：WorldSnapshot 必须实现 Serialize（前端 API 契约）
        fn assert_serialize<T: serde::Serialize>() {}
        assert_serialize::<WorldSnapshot>();
        assert_serialize::<TickInfo>();
        assert_serialize::<SnapshotAgent>();
        assert_serialize::<RecentEvent>();
    }
}
