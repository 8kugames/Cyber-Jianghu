use anyhow::{Context, Result};
use sqlx::Row;

use super::storage;
use crate::DbPool;

/// 执行单个聚合
pub async fn run_aggregation(
    db_pool: &DbPool,
    agg_name: &str,
    event_source: &str,
    group_by: &[String],
    metrics: &[String],
    jsonb_partner_fields: &[String],
    period_minutes: u64,
) -> Result<()> {
    match event_source {
        "agents" => {
            // tick 秒数来自内存 game_data registry（game_rules.yaml）；数据库中
            // 不存在 game_rules_config 表，历史上按表查询导致每轮必失败
            let real_seconds_per_tick = crate::game_data::registry_or_error()
                .map(|r| {
                    r.get()
                        .game_rules
                        .data
                        .agent_state
                        .tick
                        .real_seconds_per_tick as f64
                })
                .map_err(|e| anyhow::anyhow!("game_data registry 不可用: {}", e))?;
            collect_from_agents(
                db_pool,
                agg_name,
                group_by,
                metrics,
                period_minutes,
                real_seconds_per_tick,
            )
            .await?
        }
        "agent_action_logs" => {
            collect_from_action_logs(
                db_pool,
                agg_name,
                group_by,
                metrics,
                jsonb_partner_fields,
                period_minutes,
            )
            .await?
        }
        "agent_states" => {
            collect_from_agent_states(db_pool, agg_name, group_by, metrics, period_minutes).await?
        }
        _ => {
            tracing::warn!("未知 event_source: {}", event_source);
        }
    }
    Ok(())
}

/// 从 agents 表采集（survival_time）
///
/// `real_seconds_per_tick` 由调用方注入（run_aggregation 从 game_data registry 取；
/// 参数化使活库守卫测试可直接驱动，见 tests/sqlx_live_schema_guard_test.rs）。
pub async fn collect_from_agents(
    db_pool: &DbPool,
    agg_name: &str,
    _group_by: &[String],
    _metrics: &[String],
    period_minutes: u64,
    real_seconds_per_tick: f64,
) -> Result<()> {
    // survival_time: 统计本轮期间死亡/归隐的 agent 存活时间
    // 基于 status='dead' OR status='retired' + retired_at 在本周期内
    let period_start = chrono::Utc::now() - chrono::Duration::minutes(period_minutes as i64);
    let period_end = chrono::Utc::now();

    // CTE 将 duration 计算定义在一处，避免 AVG 和两个 PERCENTILE 中重复三遍。
    // EXTRACT(EPOCH..) 返回 numeric，须 ::float8 才能被 sqlx 解码为 f64
    // （同 emergence/loader.rs 既有规约）；server_deployment 列名为 deployed_at。
    // query! 对真实 schema 编译期校验表/列/参数/返回类型（幻表幻列在此归零）。
    let row = sqlx::query!(
        r#"
        WITH agent_durations AS (
            SELECT
                a.retired_at,
                EXTRACT(EPOCH FROM (a.retired_at - d.deployed_at + a.birth_tick * $3::float8 * interval '1 second'))::float8 as duration
            FROM agents a
            CROSS JOIN server_deployment d
            WHERE (a.status = 'dead' OR a.status = 'retired')
            AND a.retired_at IS NOT NULL
            AND a.birth_tick IS NOT NULL
            AND a.retired_at BETWEEN $1 AND $2
        )
        SELECT
            COUNT(*) as "count!",
            AVG(duration) as avg_duration,
            PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY duration) as p50_duration,
            PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY duration) as p95_duration
        FROM agent_durations
        "#,
        period_start,
        period_end,
        real_seconds_per_tick,
    )
    .fetch_one(db_pool)
    .await
    .context("查询 survival_time 失败")?;

    let count: i64 = row.count;

    if count == 0 {
        return Ok(());
    }

    let avg_duration: Option<f64> = row.avg_duration;
    let p50_duration: Option<f64> = row.p50_duration;
    let p95_duration: Option<f64> = row.p95_duration;

    let mut metrics_map = serde_json::Map::new();
    metrics_map.insert("count".to_string(), serde_json::json!(count));
    if let Some(v) = avg_duration {
        metrics_map.insert("avg_duration_seconds".to_string(), serde_json::json!(v));
    }
    if let Some(v) = p50_duration {
        metrics_map.insert("p50_duration_seconds".to_string(), serde_json::json!(v));
    }
    if let Some(v) = p95_duration {
        metrics_map.insert("p95_duration_seconds".to_string(), serde_json::json!(v));
    }

    storage::store_aggregation(
        db_pool,
        agg_name,
        period_start,
        period_end,
        None,
        None,
        &serde_json::Value::Object(metrics_map),
    )
    .await?;

    Ok(())
}

/// 从 agent_action_logs 表采集（decision_distribution, action_outcomes, interaction_activity）
async fn collect_from_action_logs(
    db_pool: &DbPool,
    agg_name: &str,
    group_by: &[String],
    metrics: &[String],
    jsonb_partner_fields: &[String],
    period_minutes: u64,
) -> Result<()> {
    let period_start = chrono::Utc::now() - chrono::Duration::minutes(period_minutes as i64);
    let period_end = chrono::Utc::now();

    match agg_name {
        "decision_distribution" => {
            let has_success_rate = metrics.iter().any(|m| m == "success_rate");
            collect_decision_distribution(
                db_pool,
                agg_name,
                period_start,
                period_end,
                has_success_rate,
                group_by,
            )
            .await?;
        }
        "action_outcomes" => {
            collect_action_outcomes(db_pool, agg_name, period_start, period_end, group_by).await?;
        }
        "interaction_activity" => {
            collect_interaction_activity(
                db_pool,
                agg_name,
                period_start,
                period_end,
                jsonb_partner_fields,
            )
            .await?;
        }
        _ => {
            tracing::warn!("未知 action_logs 聚合: {}", agg_name);
        }
    }

    Ok(())
}

/// 决策分布聚合
async fn collect_decision_distribution(
    db_pool: &DbPool,
    agg_name: &str,
    period_start: chrono::DateTime<chrono::Utc>,
    period_end: chrono::DateTime<chrono::Utc>,
    has_success_rate: bool,
    group_by: &[String],
) -> Result<()> {
    // 按 action_type 分组统计
    let rows = sqlx::query!(
        r#"
        SELECT
            action_type,
            COUNT(*) as "cnt!",
            COUNT(*) FILTER (WHERE result = 'success') as "success_cnt!"
        FROM agent_action_logs
        WHERE created_at BETWEEN $1 AND $2
        GROUP BY action_type
        ORDER BY COUNT(*) DESC
        "#,
        period_start,
        period_end,
    )
    .fetch_all(db_pool)
    .await
    .context("查询 decision_distribution 失败")?;

    for row in &rows {
        let action_type: &str = &row.action_type;
        let count: i64 = row.cnt;

        let mut metrics_map = serde_json::Map::new();
        metrics_map.insert("count".to_string(), serde_json::json!(count));

        if has_success_rate {
            let success_cnt: i64 = row.success_cnt;
            let success_rate = if count > 0 {
                success_cnt as f64 / count as f64
            } else {
                0.0
            };
            metrics_map.insert("success_rate".to_string(), serde_json::json!(success_rate));
        }

        let group_key = group_by.first().map(|s| s.as_str());
        storage::store_aggregation(
            db_pool,
            agg_name,
            period_start,
            period_end,
            group_key,
            Some(action_type),
            &serde_json::Value::Object(metrics_map),
        )
        .await?;
    }

    Ok(())
}

/// 动作结果分布聚合
async fn collect_action_outcomes(
    db_pool: &DbPool,
    agg_name: &str,
    period_start: chrono::DateTime<chrono::Utc>,
    period_end: chrono::DateTime<chrono::Utc>,
    group_by: &[String],
) -> Result<()> {
    let rows = sqlx::query!(
        r#"
        SELECT result, COUNT(*) as "cnt!"
        FROM agent_action_logs
        WHERE created_at BETWEEN $1 AND $2
        GROUP BY result
        "#,
        period_start,
        period_end,
    )
    .fetch_all(db_pool)
    .await
    .context("查询 action_outcomes 失败")?;

    for row in &rows {
        // result 列可空（无 NOT NULL 约束）：NULL 组映射为 "unknown"。
        // 旧运行期实现在出现 NULL 组时解码直接失败，宏把该缺陷提前到编译期暴露。
        let result = row.result.as_deref().unwrap_or("unknown");
        let count: i64 = row.cnt;

        let mut metrics_map = serde_json::Map::new();
        metrics_map.insert("count".to_string(), serde_json::json!(count));

        let group_key = group_by.first().map(|s| s.as_str());
        storage::store_aggregation(
            db_pool,
            agg_name,
            period_start,
            period_end,
            group_key,
            Some(result),
            &serde_json::Value::Object(metrics_map),
        )
        .await?;
    }

    Ok(())
}

/// 交互活跃度聚合（每日）
async fn collect_interaction_activity(
    db_pool: &DbPool,
    agg_name: &str,
    period_start: chrono::DateTime<chrono::Utc>,
    period_end: chrono::DateTime<chrono::Utc>,
    jsonb_partner_fields: &[String],
) -> Result<()> {
    // 统计所有有 interaction partner 的动作
    // 通过 JSONB 字段提取（如 recipient_id）
    let mut partner_conditions: Vec<String> = Vec::new();
    for field in jsonb_partner_fields {
        // SAFETY: field 来自 telemetry_config.yaml 的 jsonb_partner_fields 配置，
        // 非外部输入。PostgreSQL JSONB ->' 操作符需要字段名在 SQL 文本中，
        // 无法参数化，因此使用 format! 拼接是必要妥协。
        partner_conditions.push(format!("action_data->>'{}' IS NOT NULL", field));
    }

    if partner_conditions.is_empty() {
        // 无 partner 字段配置时，统计所有动作数
        let action_count: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) AS \"v!\" FROM agent_action_logs WHERE created_at BETWEEN $1 AND $2",
            period_start,
            period_end,
        )
        .fetch_one(db_pool)
        .await
        .context("查询 interaction_activity action_count 失败")?;

        let unique_agents: i64 = sqlx::query_scalar!(
            "SELECT COUNT(DISTINCT agent_id) AS \"v!\" FROM agent_action_logs WHERE created_at BETWEEN $1 AND $2",
            period_start,
            period_end,
        )
        .fetch_one(db_pool)
        .await
        .context("查询 interaction_activity unique_agents 失败")?;

        let mut metrics_map = serde_json::Map::new();
        metrics_map.insert("action_count".to_string(), serde_json::json!(action_count));
        metrics_map.insert(
            "unique_interacting_agents".to_string(),
            serde_json::json!(unique_agents),
        );

        storage::store_aggregation(
            db_pool,
            agg_name,
            period_start,
            period_end,
            None,
            None,
            &serde_json::Value::Object(metrics_map),
        )
        .await?;

        return Ok(());
    }

    let partner_sql = partner_conditions.join(" OR ");

    let query_str = format!(
        r#"
        SELECT
            COUNT(*) as action_count,
            COUNT(DISTINCT agent_id) as unique_agents
        FROM agent_action_logs
        WHERE created_at BETWEEN $1 AND $2
        AND ({})
        "#,
        partner_sql
    );

    let row = sqlx::query(&query_str)
        .bind(period_start)
        .bind(period_end)
        .fetch_one(db_pool)
        .await
        .context("查询 interaction_activity 失败")?;

    let action_count: i64 = row.get("action_count");
    let unique_agents: i64 = row.get("unique_agents");

    let mut metrics_map = serde_json::Map::new();
    metrics_map.insert("action_count".to_string(), serde_json::json!(action_count));
    metrics_map.insert(
        "unique_interacting_agents".to_string(),
        serde_json::json!(unique_agents),
    );

    storage::store_aggregation(
        db_pool,
        agg_name,
        period_start,
        period_end,
        None,
        None,
        &serde_json::Value::Object(metrics_map),
    )
    .await?;

    Ok(())
}

/// 从 agent_states 表采集（location_traffic）
async fn collect_from_agent_states(
    db_pool: &DbPool,
    agg_name: &str,
    group_by: &[String],
    metrics: &[String],
    period_minutes: u64,
) -> Result<()> {
    let period_start = chrono::Utc::now() - chrono::Duration::minutes(period_minutes as i64);
    let period_end = chrono::Utc::now();

    let has_agent_count = metrics.iter().any(|m| m == "agent_count");
    let has_state_count = metrics.iter().any(|m| m == "state_count");

    let select_parts = Vec::from_iter(
        [
            (has_agent_count, "COUNT(DISTINCT agent_id) as agent_count"),
            (has_state_count, "COUNT(*) as state_count"),
        ]
        .into_iter()
        .filter(|(enabled, _)| *enabled)
        .map(|(_, sql)| sql),
    );

    if select_parts.is_empty() {
        return Ok(());
    }

    let select_clause = select_parts.join(", ");

    let order_clause = if has_agent_count {
        "ORDER BY agent_count DESC"
    } else if has_state_count {
        "ORDER BY state_count DESC"
    } else {
        ""
    };

    // SAFETY: select_clause 和 order_clause 来自受控 metrics 配置（telemetry_config.yaml），
    // 非外部输入。字段名（COUNT(DISTINCT agent_id) / COUNT(*)）是固定 SQL 片段，不含用户可控值。
    let query_str = format!(
        r#"
        SELECT node_id, {}
        FROM agent_states
        WHERE created_at BETWEEN $1 AND $2
        GROUP BY node_id
        {}
        "#,
        select_clause, order_clause
    );

    let rows = sqlx::query(&query_str)
        .bind(period_start)
        .bind(period_end)
        .fetch_all(db_pool)
        .await
        .context("查询 location_traffic 失败")?;

    for row in &rows {
        let node_id: String = row.get("node_id");
        let mut metrics_map = serde_json::Map::new();

        if has_agent_count {
            let agent_count: i64 = row.get("agent_count");
            metrics_map.insert("agent_count".to_string(), serde_json::json!(agent_count));
        }
        if has_state_count {
            let state_count: i64 = row.get("state_count");
            metrics_map.insert("state_count".to_string(), serde_json::json!(state_count));
        }

        let group_key = group_by.first().map(|s| s.as_str());
        storage::store_aggregation(
            db_pool,
            agg_name,
            period_start,
            period_end,
            group_key,
            Some(&node_id),
            &serde_json::Value::Object(metrics_map),
        )
        .await?;
    }

    Ok(())
}
