//! 单次 run 编排 (Task 7 完整实现, 本 Task 先放 db 查询)

use std::collections::HashMap;

use cyber_jianghu_protocol::SoulCycleMetadata;
use sqlx::PgPool;
use uuid::Uuid;

/// 从 DB 查每个 (agent_id, tick_id) 的 soul_cycle_metadata (取最大 pipe_seq).
///
/// SQL 对齐 scripts/build_sft_data.py:84-92 (DISTINCT ON + pipe_seq DESC).
/// IN 子句用 UNNEST($1::uuid[], $2::bigint[]) 避免 sqlx 复合类型映射 (spec §5.3.1).
/// statement_timeout 用 SET LOCAL 在短事务内 (防 GUC 泄漏, spec §5.3.1).
pub async fn fetch_soul_cycle_metadata(
    pool: &PgPool,
    keys: &[(Uuid, i64)],
    statement_timeout_secs: u64,
) -> anyhow::Result<HashMap<(Uuid, i64), SoulCycleMetadata>> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let agent_ids: Vec<Uuid> = keys.iter().map(|(a, _)| *a).collect();
    let tick_ids: Vec<i64> = keys.iter().map(|(_, t)| *t).collect();

    let mut tx = pool.begin().await?;
    sqlx::query(&format!(
        "SET LOCAL statement_timeout = '{}s'",
        statement_timeout_secs
    ))
    .execute(&mut *tx)
    .await
    .context("SET LOCAL statement_timeout 失败")?;

    let rows = sqlx::query_as::<_, SoulCycleRow>(
        r#"
        SELECT DISTINCT ON (agent_id, tick_id)
               agent_id, tick_id, soul_cycle_metadata
        FROM agent_action_logs
        WHERE soul_cycle_metadata IS NOT NULL
          AND (agent_id, tick_id) IN (
              SELECT * FROM UNNEST($1::uuid[], $2::bigint[])
          )
        ORDER BY agent_id, tick_id, pipe_seq DESC
        "#,
    )
    .bind(&agent_ids)
    .bind(&tick_ids)
    .fetch_all(&mut *tx)
    .await
    .context("查询 soul_cycle_metadata 失败")?;

    tx.commit().await.context("提交只读事务失败")?;

    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        if let Some(metadata_value) = row.soul_cycle_metadata {
            match serde_json::from_value::<SoulCycleMetadata>(metadata_value) {
                Ok(m) => {
                    map.insert((row.agent_id, row.tick_id), m);
                }
                Err(e) => {
                    tracing::warn!(
                        agent_id = %row.agent_id,
                        tick_id = row.tick_id,
                        "解析 soul_cycle_metadata 失败: {}",
                        e
                    );
                }
            }
        }
    }
    Ok(map)
}

#[derive(sqlx::FromRow)]
struct SoulCycleRow {
    agent_id: Uuid,
    tick_id: i64,
    soul_cycle_metadata: Option<serde_json::Value>,
}

use anyhow::Context as _;
