// ============================================================================
// 涌现检测数据加载层（sqlx 查询，与判定逻辑分离）
// ============================================================================
//
// 移植自 scripts/detect_emergence.py 的 fetch_window / fetch_health。
// 只读查询，不修改任何数据。
// ============================================================================

use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::Row;
use uuid::Uuid;

use super::detector::ActionRow;
use super::HealthMetrics;

/// 读取时间窗口内的动作流 + agent 名字映射。
///
/// thought_text = COALESCE(顶层 thought_log, metadata 嵌套 thought_log)。
/// node_id LEFT JOIN agent_states（不保证每 tick 有快照）。
pub async fn fetch_window(
    db_pool: &crate::db::DbPool,
    tick_start: i64,
    tick_end: i64,
) -> Result<(Vec<ActionRow>, HashMap<Uuid, String>)> {
    let rows = sqlx::query(
        r#"
        SELECT l.tick_id,
               l.agent_id,
               l.pipe_seq,
               l.action_type,
               l.result,
               l.action_data,
               COALESCE(l.thought_log,
                        l.soul_cycle_metadata->'cycles'->0->'renhun'->>'thought_log') AS thought_text,
               s.node_id
        FROM agent_action_logs l
        LEFT JOIN agent_states s
          ON s.agent_id = l.agent_id AND s.tick_id = l.tick_id
        WHERE l.tick_id BETWEEN $1 AND $2
        ORDER BY l.tick_id, l.agent_id, l.pipe_seq
        "#,
    )
    .bind(tick_start)
    .bind(tick_end)
    .fetch_all(db_pool)
    .await
    .context("查询动作流窗口失败")?;

    let action_rows: Vec<ActionRow> = rows
        .into_iter()
        .map(|row| {
            let action_data: serde_json::Value = row
                .get::<Option<serde_json::Value>, _>("action_data")
                .unwrap_or(serde_json::Value::Null);
            ActionRow {
                tick_id: row.get("tick_id"),
                agent_id: row.get("agent_id"),
                pipe_seq: row.get::<i32, _>("pipe_seq"),
                action_type: row.get("action_type"),
                result: row.get::<Option<String>, _>("result").unwrap_or_default(),
                action_data,
                thought_text: row.get("thought_text"),
                node_id: row.get("node_id"),
            }
        })
        .collect();

    // agent 名字
    let name_rows = sqlx::query("SELECT agent_id, name FROM agents")
        .fetch_all(db_pool)
        .await
        .context("查询 agent 名字失败")?;
    let agent_names: HashMap<Uuid, String> = name_rows
        .into_iter()
        .map(|row| {
            let id: Uuid = row.get("agent_id");
            let name: String = row.get("name");
            (id, name)
        })
        .collect();

    Ok((action_rows, agent_names))
}

/// 当前世界 tick_id（用于默认窗口回溯）。
pub async fn current_max_tick(db_pool: &crate::db::DbPool) -> Result<i64> {
    let row = sqlx::query("SELECT COALESCE(MAX(tick_id), 0) as max_tick FROM agent_action_logs")
        .fetch_one(db_pool)
        .await
        .context("查询 max tick_id 失败")?;
    let max_tick: i64 = row.get("max_tick");
    Ok(max_tick)
}

/// 读取 MVP §6.1.1/§6.1.2 健康度。
pub async fn fetch_health(
    db_pool: &crate::db::DbPool,
    tick_start: i64,
    tick_end: i64,
    supply_actions: &[String],
    min_survivors: i32,
    min_supply_count: i32,
) -> Result<HealthMetrics> {
    use crate::emergence::HealthMetrics;
    let mut h = HealthMetrics {
        min_survivors_required: min_survivors,
        min_supply_required: min_supply_count,
        ..Default::default()
    };

    // tick 完成率（tick_logs.status 聚合）+ 连续运行跨度
    // 注意：EXTRACT(EPOCH FROM ...) 返回 PG numeric 类型，sqlx 无法直接解码为 f64，
    // 必须显式 ::float8 cast 成 double precision。
    let tick_rows = sqlx::query(
        r#"
        SELECT status,
               COUNT(*) as cnt,
               COALESCE(EXTRACT(EPOCH FROM (COALESCE(MAX(completed_at), MAX(started_at)) - MIN(started_at)))::float8, 0.0) as span
        FROM tick_logs
        WHERE tick_id BETWEEN $1 AND $2
        GROUP BY status
        "#,
    )
    .bind(tick_start)
    .bind(tick_end)
    .fetch_all(db_pool)
    .await
    .context("查询 tick_logs 健康度失败")?;

    for row in &tick_rows {
        let status: String = row.get("status");
        let cnt: i64 = row.get("cnt");
        // span 可能为 NULL（空表/无 completed_at），用 Option 防 panic
        let span: f64 = row.get::<Option<f64>, _>("span").unwrap_or(0.0);
        h.ticks_total += cnt;
        match status.as_str() {
            "completed" => h.ticks_completed = cnt,
            "failed" => h.ticks_failed = cnt,
            "running" => h.ticks_running = cnt,
            _ => {}
        }
        if span > h.continuous_run_seconds {
            h.continuous_run_seconds = span;
        }
    }
    h.tick_completion_rate = if h.ticks_total > 0 {
        h.ticks_completed as f64 / h.ticks_total as f64
    } else {
        0.0
    };

    // 窗口末点存活数（取窗口内最大 tick 的 agent_states 快照）
    let alive_rows = sqlx::query(
        r#"
        SELECT COUNT(*) as alive_cnt
        FROM agent_states s
        WHERE s.is_alive = true
          AND s.tick_id = (SELECT MAX(tick_id) FROM agent_states WHERE tick_id BETWEEN $1 AND $2)
        "#,
    )
    .bind(tick_start)
    .bind(tick_end)
    .fetch_all(db_pool)
    .await
    .context("查询存活数失败")?;
    h.agents_alive = alive_rows
        .first()
        .map(|r| r.get::<i64, _>("alive_cnt") as i32)
        .unwrap_or(0);
    h.survivors_pass = h.agents_alive >= min_survivors;

    // 应参与 agent 数（active+alive，用于超时率近似的分母）
    let expected_rows = sqlx::query(
        r#"
        SELECT COUNT(DISTINCT s.agent_id) as expected_cnt
        FROM agent_states s
        INNER JOIN agents a ON s.agent_id = a.agent_id
        WHERE s.is_alive = true AND a.status = 'active'
          AND s.tick_id = (SELECT MAX(tick_id) FROM agent_states WHERE tick_id BETWEEN $1 AND $2)
        "#,
    )
    .bind(tick_start)
    .bind(tick_end)
    .fetch_all(db_pool)
    .await
    .context("查询应参与 agent 数失败")?;
    h.agents_expected = expected_rows
        .first()
        .map(|r| r.get::<i64, _>("expected_cnt") as i32)
        .unwrap_or(0);

    // 实际有动作提交的 agent 数（超时率近似的分子）
    let submitted_rows = sqlx::query(
        r#"
        SELECT COUNT(DISTINCT agent_id) as submitted_cnt
        FROM agent_action_logs
        WHERE tick_id BETWEEN $1 AND $2
        "#,
    )
    .bind(tick_start)
    .bind(tick_end)
    .fetch_all(db_pool)
    .await
    .context("查询已提交 agent 数失败")?;
    h.agents_submitted = submitted_rows
        .first()
        .map(|r| r.get::<i64, _>("submitted_cnt") as i32)
        .unwrap_or(0);
    // 超时率近似 = 1 − 已提交/应参与（标注为近似，非 MVP 字面30秒墙钟）
    h.timeout_rate_approx = if h.agents_expected > 0 {
        1.0 - (h.agents_submitted as f64 / h.agents_expected as f64)
    } else {
        0.0
    };

    // 每存活 agent 补给次数
    if !supply_actions.is_empty() {
        // 存活 agent id 集合
        let alive_ids: Vec<Uuid> = sqlx::query(
            r#"
            SELECT DISTINCT s.agent_id
            FROM agent_states s
            INNER JOIN agents a ON s.agent_id = a.agent_id
            WHERE s.is_alive = true AND a.status = 'active'
              AND s.tick_id = (SELECT MAX(tick_id) FROM agent_states WHERE tick_id BETWEEN $1 AND $2)
            "#,
        )
        .bind(tick_start)
        .bind(tick_end)
        .fetch_all(db_pool)
        .await
        .context("查询存活 agent id 失败")?
        .into_iter()
        .map(|r| r.get::<Uuid, _>("agent_id"))
        .collect();

        // 补给次数
        let placeholders: String = (0..supply_actions.len())
            .map(|i| format!("${}", i + 3))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            r#"
            SELECT agent_id, COUNT(*) as cnt
            FROM agent_action_logs
            WHERE tick_id BETWEEN $1 AND $2
              AND action_type IN ({placeholders})
              AND result = 'success'
            GROUP BY agent_id
            "#,
        );
        let mut q = sqlx::query(&sql).bind(tick_start).bind(tick_end);
        for a in supply_actions {
            q = q.bind(a);
        }
        let supply_map: HashMap<Uuid, i64> = q
            .fetch_all(db_pool)
            .await
            .context("查询补给次数失败")?
            .into_iter()
            .map(|r| (r.get::<Uuid, _>("agent_id"), r.get::<i64, _>("cnt")))
            .collect();

        let mut per_agent: HashMap<Uuid, i32> = HashMap::new();
        for id in &alive_ids {
            per_agent.insert(*id, supply_map.get(id).copied().unwrap_or(0) as i32);
        }
        h.per_agent_supply = per_agent;
        h.supply_pass = !alive_ids.is_empty()
            && alive_ids
                .iter()
                .all(|id| *h.per_agent_supply.get(id).unwrap_or(&0) >= min_supply_count);
    }

    Ok(h)
}
