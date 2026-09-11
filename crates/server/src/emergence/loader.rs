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

use super::HealthMetrics;
use super::detector::ActionRow;

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

/// 读取 MVP 运行稳定性/生存能力/行为多样性健康度。
#[allow(clippy::too_many_arguments)]
pub async fn fetch_health(
    db_pool: &crate::db::DbPool,
    tick_start: i64,
    tick_end: i64,
    supply_actions: &[String],
    min_survivors: i32,
    min_supply_count: i32,
    max_top_share: f64,
    satiation_urgent_below: i32,
) -> Result<HealthMetrics> {
    use crate::emergence::{BehaviorStat, HealthMetrics};
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

    // MVP 行为多样性：窗口内 per-agent 决策动作分布熵（含被拒决策，
    let sat_rows = sqlx::query(
            r#"
            SELECT agent_id, COALESCE((attributes->>'satiation')::float8, 999.0) as satiation
            FROM agent_states s
            WHERE s.tick_id = (SELECT MAX(tick_id) FROM agent_states WHERE tick_id BETWEEN $1 AND $2)
            "#,
        )
        .bind(tick_start)
        .bind(tick_end)
        .fetch_all(db_pool)
        .await
        .context("查询饱食度失败")?;
    let satiation: HashMap<Uuid, f64> = sat_rows
        .into_iter()
        .map(|r| (r.get::<Uuid, _>("agent_id"), r.get::<f64, _>("satiation")))
        .collect();
    h.per_agent_satiation = satiation.clone();

    // MVP 行为多样性：窗口内 per-agent 最频动作占比统计（含被拒决策，
    // agent_action_logs 在事务内入库覆盖全部提交）。
    // 回答的问题只有一个：agent 是否卡在单一动作循环里？
    // 判读：top_share ≥ max_top_share 且生存紧迫（饱食度低）→ 卡死循环；
    //       饱食安稳下的高占比属合理惰性，豁免。
    {
        let dist_rows = sqlx::query(
            r#"
            SELECT agent_id, action_type, COUNT(*) as cnt
            FROM agent_action_logs
            WHERE tick_id BETWEEN $1 AND $2
            GROUP BY agent_id, action_type
            "#,
        )
        .bind(tick_start)
        .bind(tick_end)
        .fetch_all(db_pool)
        .await
        .context("查询行为分布失败")?;

        let mut dist: HashMap<Uuid, Vec<(String, i64)>> = HashMap::new();
        for r in &dist_rows {
            let aid: Uuid = r.get("agent_id");
            let at: String = r.get("action_type");
            let cnt: i64 = r.get("cnt");
            dist.entry(aid).or_default().push((at, cnt));
        }

        // 窗口末点快照的饱食度（attributes->>'satiation'，无快照视为安稳）
        let sat_rows = sqlx::query(
            r#"
            SELECT agent_id, COALESCE((attributes->>'satiation')::float8, 999.0) as satiation
            FROM agent_states s
            WHERE s.tick_id = (SELECT MAX(tick_id) FROM agent_states WHERE tick_id BETWEEN $1 AND $2)
            "#,
        )
        .bind(tick_start)
        .bind(tick_end)
        .fetch_all(db_pool)
        .await
        .context("查询饱食度失败")?;
        let satiation: HashMap<Uuid, f64> = sat_rows
            .into_iter()
            .map(|r| (r.get::<Uuid, _>("agent_id"), r.get::<f64, _>("satiation")))
            .collect();
        h.per_agent_satiation = satiation.clone();

        let min_decisions = 5i64; // 样本不足不判读，避免误报
        let mut fail_agents: Vec<Uuid> = Vec::new();
        for (aid, dist_list) in &dist {
            let total: i64 = dist_list.iter().map(|(_, c)| c).sum();
            let (top_action, top_cnt) = dist_list
                .iter()
                .max_by_key(|(_, c)| *c)
                .map(|(a, c)| (a.clone(), *c))
                .unwrap_or_default();
            let top_share = if total > 0 {
                top_cnt as f64 / total as f64
            } else {
                0.0
            };
            let sat = satiation.get(aid).copied().unwrap_or(999.0);
            // 豁免：饱食安稳（≥ satiation_urgent_below）时的高占比属合理惰性
            let exempted = top_share >= max_top_share && sat >= satiation_urgent_below as f64;
            let stuck = top_share >= max_top_share && !exempted && total >= min_decisions;
            let stat = BehaviorStat {
                top_action,
                top_share,
                distinct_actions: dist_list.len(),
                total_decisions: total,
                satiation: sat,
                exempted,
            };
            h.per_agent_behavior.insert(*aid, stat);
            if stuck {
                fail_agents.push(*aid);
            }
        }
        h.behavior_fail_agents = fail_agents;
        h.behavior_pass = h.behavior_fail_agents.is_empty();
    }

    Ok(h)
}
