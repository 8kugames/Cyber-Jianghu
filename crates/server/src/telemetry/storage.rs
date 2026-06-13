use anyhow::{Context, Result};
use sqlx::Row;

use super::TelemetryRow;
use crate::DbPool;

/// 存储一条聚合结果
pub async fn store_aggregation(
    db_pool: &DbPool,
    aggregation_name: &str,
    period_start: chrono::DateTime<chrono::Utc>,
    period_end: chrono::DateTime<chrono::Utc>,
    group_by_key: Option<&str>,
    group_by_value: Option<&str>,
    metrics: &serde_json::Value,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO telemetry_aggregations
            (aggregation_name, period_start, period_end, group_by_key, group_by_value, metrics)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(aggregation_name)
    .bind(period_start)
    .bind(period_end)
    .bind(group_by_key)
    .bind(group_by_value)
    .bind(metrics)
    .execute(db_pool)
    .await
    .context("存储遥测聚合失败")?;

    Ok(())
}

/// 查询聚合结果（按名称和时间范围）
pub async fn query_aggregations(
    db_pool: &DbPool,
    aggregation_name: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<TelemetryRow>> {
    let rows = sqlx::query(
        r#"
        SELECT id, aggregation_name, period_start, period_end,
               group_by_key, group_by_value, metrics, created_at
        FROM telemetry_aggregations
        WHERE aggregation_name = $1
        ORDER BY period_start DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(aggregation_name)
    .bind(limit)
    .bind(offset)
    .fetch_all(db_pool)
    .await
    .context("查询遥测聚合失败")?;

    let results: Vec<TelemetryRow> = rows
        .iter()
        .map(|r| TelemetryRow {
            id: r.get("id"),
            aggregation_name: r.get("aggregation_name"),
            period_start: r.get("period_start"),
            period_end: r.get("period_end"),
            group_by_key: r.get("group_by_key"),
            group_by_value: r.get("group_by_value"),
            metrics: r.get("metrics"),
            created_at: r.get("created_at"),
        })
        .collect();

    Ok(results)
}

/// 获取可用的聚合名称列表
pub async fn list_aggregation_names(db_pool: &DbPool) -> Result<Vec<String>> {
    let rows: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT aggregation_name
        FROM telemetry_aggregations
        ORDER BY aggregation_name
        "#,
    )
    .fetch_all(db_pool)
    .await
    .context("查询聚合名称列表失败")?;

    Ok(rows)
}

/// 获取最近的聚合时间（用于增量模式）
pub async fn get_latest_aggregation_time(
    db_pool: &DbPool,
    aggregation_name: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let row: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        r#"
        SELECT MAX(period_end)
        FROM telemetry_aggregations
        WHERE aggregation_name = $1
        "#,
    )
    .bind(aggregation_name)
    .fetch_optional(db_pool)
    .await
    .context("查询最新聚合时间失败")?;

    Ok(row)
}
