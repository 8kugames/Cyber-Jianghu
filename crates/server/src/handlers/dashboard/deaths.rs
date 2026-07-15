// ============================================================================
// 死亡时间线端点
// ============================================================================
//
// GET /api/dashboard/deaths
//
// 返回游戏世界中已死亡 Agent 的时间线（agent_id, name, death_tick, cause, narrative）。
//
// 数据来源：
//   - agents 表 status='dead' 的记录（retired_at 作为死亡时间戳，birth_tick 推算 death_tick）
//   - agent_action_logs 表中该 agent 死亡时刻附近的失败动作（action_type='攻击' AND result='failed'），
//     取其 narrative / result_message 作为死亡叙事，action_type 作为死因。
//
// 与 /api/dashboard/agents/dead 的区别：后者仅列出当前死亡的 agent（无时间/原因/叙事），
// 本端点面向"死亡事件序列"叙事展示。
//
// 可选查询参数：
//   ?limit=N        最多返回条数（默认 50，上限 200）
//   ?tick_from=最小 tick_id（默认不过滤，按 death_tick 过滤）
// ============================================================================

use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::AppState;

// ============================================================================
// 查询参数
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct DeathsQuery {
    /// 最多返回条数（默认 50，上限 200）
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// 最小 death_tick（默认不过滤）
    pub tick_from: Option<i64>,
}

fn default_limit() -> i64 {
    50
}

// ============================================================================
// 响应结构
// ============================================================================

/// 死亡时间线响应
#[derive(Debug, Serialize)]
pub struct DeathsResponse {
    /// 返回的死亡事件条数
    pub count: usize,
    /// 死亡事件（按死亡时间倒序，最新的在前）
    pub deaths: Vec<DeathEntry>,
}

/// 单条死亡事件
#[derive(Debug, Serialize)]
pub struct DeathEntry {
    pub agent_id: Uuid,
    /// 死亡 Agent 名称（agents.name，缺失 → "unknown"）
    pub name: String,
    /// 死亡 tick（由 retired_at 反推：retired_at 距部署时刻 / real_seconds_per_tick + birth_tick）
    pub death_tick: Option<i64>,
    /// 出生 tick（NULL = 不朽/未知）
    pub birth_tick: Option<i64>,
    /// 死亡时间戳（agents.retired_at）
    pub death_at: Option<chrono::DateTime<chrono::Utc>>,
    /// 死因代码（最近一条失败战斗动作的 action_type；缺失 → "unknown"）
    pub cause: String,
    /// 死亡叙事（最近一条失败战斗动作的 narrative / result_message；缺失 → agents.biography）
    pub narrative: Option<String>,
    /// 死亡时所在节点（最新 agent_states.node_id 近似；NULL → "unknown"）
    pub location: String,
}

// ============================================================================
// Handler
// ============================================================================

/// GET /api/dashboard/deaths
///
/// 死亡时间线：从 agents 表查 status='dead'，对每个死亡 agent 取死亡时刻附近的失败动作
/// （action_type='攻击' AND result='failed'）作为死因/叙事来源。
pub async fn get_deaths(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DeathsQuery>,
) -> Json<DeathsResponse> {
    let limit = query.limit.clamp(1, 200);

    // 查 status='dead' 的 agent，LATERAL JOIN 取死亡时刻附近最近一条失败战斗动作
    // 以及最新 agent_states.node_id 近似死亡位置。
    //
    // death_tick 推算：server_deployment.deployment_time + birth_tick * tick_duration 为出生时刻，
    // retired_at 与之差除以 tick_duration 即死亡 tick。
    // 为避免引入 game_rules_config JOIN 导致整查询失败，real_seconds_per_tick 与 deployment_time
    // 单独取值，death_tick 在 Rust 层推算。tick_from 过滤基于 death_tick（推算后），亦在 Rust 层完成。
    let sql = r#"
        WITH dead_agents AS (
            SELECT
                a.agent_id,
                a.name,
                a.birth_tick,
                a.retired_at,
                a.biography,
                a.created_at
            FROM agents a
            WHERE a.status = 'dead'
        )
        SELECT
            da.agent_id,
            da.name,
            da.birth_tick,
            da.retired_at,
            da.biography,
            COALESCE(s.node_id, 'unknown') as location,
            COALESCE(cl.action_type, 'unknown') as cause,
            COALESCE(cl.narrative, cl.result_message) as narrative
        FROM dead_agents da
        LEFT JOIN LATERAL (
            SELECT node_id FROM agent_states
            WHERE agent_states.agent_id = da.agent_id
            ORDER BY tick_id DESC LIMIT 1
        ) s ON true
        LEFT JOIN LATERAL (
            SELECT action_type, narrative, result_message, created_at
            FROM agent_action_logs
            WHERE agent_action_logs.agent_id = da.agent_id
              AND agent_action_logs.result = 'failed'
              AND agent_action_logs.action_type = '攻击'
            ORDER BY agent_action_logs.created_at DESC
            LIMIT 1
        ) cl ON true
        WHERE da.retired_at IS NOT NULL
        ORDER BY da.retired_at DESC
        LIMIT $1
        "#;

    // tick_from 过滤基于推算后的 death_tick，需先取配置在 Rust 层过滤；
    // 为避免过滤后条数不足，SQL 层取 4 倍 limit 预留余量。
    let sql_limit = if query.tick_from.is_some() {
        limit * 4
    } else {
        limit
    };

    // death_tick 由 retired_at / birth_tick / real_seconds_per_tick 推算；为避免在此 SQL 中
    // 强依赖 game_rules_config（可能为空导致整个查询失败），改在 Rust 层取 real_seconds_per_tick 后推算。
    let real_seconds_per_tick: f64 =
        sqlx::query_scalar("SELECT real_seconds_per_tick::float8 FROM game_rules_config LIMIT 1")
            .fetch_optional(&state.db_pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(1.0);

    let deployment_time: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT MIN(deployment_time) FROM server_deployment")
            .fetch_optional(&state.db_pool)
            .await
            .ok()
            .flatten();

    let rows = match sqlx::query(sql)
        .bind(sql_limit)
        .fetch_all(&state.db_pool)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("查询 deaths 失败: {}", e);
            return Json(DeathsResponse {
                count: 0,
                deaths: Vec::new(),
            });
        }
    };

    let mut deaths: Vec<DeathEntry> = rows
        .into_iter()
        .map(|row| {
            let retired_at: Option<chrono::DateTime<chrono::Utc>> = row.get("retired_at");
            let birth_tick: Option<i64> = row.get("birth_tick");

            // 推算 death_tick = birth_tick + (retired_at - origin) / tick_duration
            let death_tick = match (retired_at, birth_tick, deployment_time) {
                (Some(rt), Some(bt), Some(dep)) => {
                    let dur = (rt - dep).num_milliseconds() as f64 / 1000.0;
                    let ticks = (dur / real_seconds_per_tick).round() as i64;
                    Some(bt + ticks)
                }
                _ => None,
            };

            let narrative: Option<String> = row
                .get::<Option<String>, _>("narrative")
                .or_else(|| row.get::<Option<String>, _>("biography"));

            DeathEntry {
                agent_id: row.get("agent_id"),
                name: row
                    .get::<Option<String>, _>("name")
                    .unwrap_or_else(|| "unknown".to_string()),
                death_tick,
                birth_tick,
                death_at: retired_at,
                cause: row.get("cause"),
                narrative,
                location: row.get("location"),
            }
        })
        .collect();

    // tick_from 过滤基于推算后的 death_tick（无法推算的视为不匹配）
    if let Some(tick_from) = query.tick_from {
        deaths.retain(|d| d.death_tick.map_or(false, |t| t >= tick_from));
    }
    deaths.truncate(limit as usize);

    let count = deaths.len();
    Json(DeathsResponse { count, deaths })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limit_is_50() {
        assert_eq!(default_limit(), 50);
    }

    #[test]
    fn deaths_response_is_serialize() {
        fn assert_serialize<T: serde::Serialize>() {}
        assert_serialize::<DeathsResponse>();
        assert_serialize::<DeathEntry>();
    }
}
