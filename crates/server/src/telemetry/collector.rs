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
            collect_from_agents(db_pool, agg_name, group_by, metrics, period_minutes).await?
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
async fn collect_from_agents(
    db_pool: &DbPool,
    agg_name: &str,
    _group_by: &[String],
    _metrics: &[String],
    period_minutes: u64,
) -> Result<()> {
    // survival_time: 统计本轮期间死亡/归隐的 agent 存活时间
    // 基于 status='dead' OR status='retired' + retired_at 在本周期内
    let period_start = chrono::Utc::now() - chrono::Duration::minutes(period_minutes as i64);
    let period_end = chrono::Utc::now();

    let rows = sqlx::query(
        r#"
        SELECT
            COUNT(*) as count,
            AVG(EXTRACT(EPOCH FROM (a.retired_at - d.deployment_time + a.birth_tick * t.real_seconds_per_tick * interval '1 second')))
            as avg_duration,
            PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY
                EXTRACT(EPOCH FROM (a.retired_at - d.deployment_time + a.birth_tick * t.real_seconds_per_tick * interval '1 second'))
            ) as p50_duration,
            PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY
                EXTRACT(EPOCH FROM (a.retired_at - d.deployment_time + a.birth_tick * t.real_seconds_per_tick * interval '1 second'))
            ) as p95_duration
        FROM agents a
        CROSS JOIN server_deployment d
        CROSS JOIN (SELECT real_seconds_per_tick FROM game_rules_config LIMIT 1) t
        WHERE (a.status = 'dead' OR a.status = 'retired')
        AND a.retired_at IS NOT NULL
        AND a.birth_tick IS NOT NULL
        AND a.retired_at BETWEEN $1 AND $2
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询 survival_time 失败")?;

    if rows.is_empty() {
        return Ok(());
    }

    let row = &rows[0];
    let count: i64 = row.try_get("count").unwrap_or(0);

    if count == 0 {
        return Ok(());
    }

    let avg_duration: Option<f64> = row.try_get("avg_duration").ok();
    let p50_duration: Option<f64> = row.try_get("p50_duration").ok();
    let p95_duration: Option<f64> = row.try_get("p95_duration").ok();

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
    _group_by: &[String],
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
            )
            .await?;
        }
        "action_outcomes" => {
            collect_action_outcomes(db_pool, agg_name, period_start, period_end).await?;
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
) -> Result<()> {
    // 按 action_type 分组统计
    let rows = sqlx::query(
        r#"
        SELECT
            action_type,
            COUNT(*) as cnt,
            COUNT(*) FILTER (WHERE result = 'success') as success_cnt
        FROM agent_action_logs
        WHERE created_at BETWEEN $1 AND $2
        GROUP BY action_type
        ORDER BY cnt DESC
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询 decision_distribution 失败")?;

    for row in &rows {
        let action_type: String = row.get("action_type");
        let count: i64 = row.get("cnt");

        let mut metrics_map = serde_json::Map::new();
        metrics_map.insert("count".to_string(), serde_json::json!(count));

        if has_success_rate {
            let success_cnt: i64 = row.get("success_cnt");
            let success_rate = if count > 0 {
                success_cnt as f64 / count as f64
            } else {
                0.0
            };
            metrics_map.insert("success_rate".to_string(), serde_json::json!(success_rate));
        }

        storage::store_aggregation(
            db_pool,
            agg_name,
            period_start,
            period_end,
            Some("action_type"),
            Some(&action_type),
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
) -> Result<()> {
    let rows = sqlx::query(
        r#"
        SELECT result, COUNT(*) as cnt
        FROM agent_action_logs
        WHERE created_at BETWEEN $1 AND $2
        GROUP BY result
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询 action_outcomes 失败")?;

    for row in &rows {
        let result: String = row.get("result");
        let count: i64 = row.get("cnt");

        let mut metrics_map = serde_json::Map::new();
        metrics_map.insert("count".to_string(), serde_json::json!(count));

        storage::store_aggregation(
            db_pool,
            agg_name,
            period_start,
            period_end,
            Some("result"),
            Some(&result),
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
        partner_conditions.push(format!("action_data->>'{}' IS NOT NULL", field));
    }

    if partner_conditions.is_empty() {
        // 无 partner 字段配置时，统计所有动作数
        let action_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_action_logs WHERE created_at BETWEEN $1 AND $2",
        )
        .bind(period_start)
        .bind(period_end)
        .fetch_one(db_pool)
        .await
        .context("查询 interaction_activity action_count 失败")?;

        let unique_agents: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT agent_id) FROM agent_action_logs WHERE created_at BETWEEN $1 AND $2",
        )
        .bind(period_start)
        .bind(period_end)
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
    _group_by: &[String],
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

        storage::store_aggregation(
            db_pool,
            agg_name,
            period_start,
            period_end,
            Some("node_id"),
            Some(&node_id),
            &serde_json::Value::Object(metrics_map),
        )
        .await?;
    }

    Ok(())
}
