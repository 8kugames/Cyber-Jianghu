// ============================================================================
// C1: Agent 关系图谱存储操作
// ============================================================================
//
// 全量快照同步策略（与 DailySummary 同步链路对齐）：
// 每游戏日结束时 agent 把完整关系快照上报，server 在事务内
// DELETE 旧数据 + INSERT 新数据，天然幂等。
//
// 表结构见 migrations/022_agent_relationships.sql。
// 时间戳全部使用 BIGINT Unix 毫秒（与 protocol 契约一致）。
//
// 镜像 agent 端 crates/agent/src/component/social/relationship.rs 的语义。

use anyhow::{Context, Result};
use sqlx::{PgPool, Row};
use tracing::debug;
use uuid::Uuid;

use cyber_jianghu_protocol::types::{
    RelationshipKeyEvent, RelationshipMemory,
};

// ============================================================================
// 写入（全量快照覆盖）
// ============================================================================

/// 全量覆盖某个 source agent 的关系快照
///
/// 在单个事务内执行：
/// 1. 删除该 source 的所有 key_events（CASCADE 会跟随，但显式更稳）
/// 2. 删除该 source 的所有 agent_relationships 行
/// 3. 重新插入传入的 relationships + key_events
///
/// 幂等：同一 game_day 多次调用结果一致（DELETE+INSERT）。
///
/// 镜像 agent 端 `RelationshipStore::upsert_relationship` 的 DELETE-then-INSERT 语义，
/// 但作用域为"一个 source 的全部关系"（全量覆盖）。
pub async fn upsert_relationship_snapshot(
    pool: &PgPool,
    source_agent_id: Uuid,
    game_day: i64,
    relationships: &[RelationshipMemory],
    synced_at: i64,
) -> Result<()> {
    let mut tx = pool
        .begin()
        .await
        .context("开启关系快照事务失败")?;

    // 1. 清空旧数据（CASCADE 处理 key_events，显式 DELETE 仅作防御）
    sqlx::query(
        "DELETE FROM agent_relationships WHERE source_agent_id = $1",
    )
    .bind(source_agent_id)
    .execute(&mut *tx)
    .await
    .context("清理旧 agent_relationships 失败")?;

    // 2. 插入关系行 + 事件
    for rel in relationships {
        sqlx::query(
            r#"
            INSERT INTO agent_relationships
                (source_agent_id, target_agent_id, target_name, favorability,
                 last_interaction_tick, synced_at, self_description, description_tick)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(source_agent_id)
        .bind(rel.target_agent_id)
        .bind(&rel.target_name)
        .bind(rel.favorability)
        .bind(rel.last_interaction_tick)
        .bind(synced_at)
        .bind(&rel.self_description)
        .bind(rel.description_tick)
        .execute(&mut *tx)
        .await
        .context("插入 agent_relationships 失败")?;

        for ev in &rel.key_events {
            sqlx::query(
                r#"
                INSERT INTO agent_relationship_key_events
                    (source_agent_id, target_agent_id, tick_id, event_type,
                     description, favorability_delta, event_timestamp)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                "#,
            )
            .bind(source_agent_id)
            .bind(rel.target_agent_id)
            .bind(ev.tick_id)
            .bind(&ev.event_type)
            .bind(&ev.description)
            .bind(ev.favorability_delta)
            .bind(ev.timestamp)
            .execute(&mut *tx)
            .await
            .context("插入 agent_relationship_key_events 失败")?;
        }
    }

    tx.commit().await.context("提交关系快照事务失败")?;

    debug!(
        "关系快照已覆盖: source={}, game_day={}, relationships={}",
        source_agent_id,
        game_day,
        relationships.len()
    );

    Ok(())
}

// ============================================================================
// 读取（供 dashboard 端点使用）
// ============================================================================

/// 把数据库行重新组装成 protocol RelationshipMemory（含 key_events）
///
/// 返回顺序：relationships 按 synced_at DESC, target_name ASC；
/// key_events 按 tick_id ASC（与 agent 端 FIFO 语义一致，便于前端按时间线呈现）。
async fn fetch_relationships(
    pool: &PgPool,
    source_filter: Option<Uuid>,
) -> Result<Vec<(Uuid, RelationshipMemory)>> {
    // 1. 拉取所有关系行
    let rel_rows = if let Some(src) = source_filter {
        sqlx::query(
            r#"
            SELECT source_agent_id, target_agent_id, target_name, favorability,
                   last_interaction_tick, synced_at, self_description, description_tick
            FROM agent_relationships
            WHERE source_agent_id = $1
            ORDER BY synced_at DESC, target_name ASC
            "#,
        )
        .bind(src)
        .fetch_all(pool)
        .await
        .context("查询 agent_relationships 失败")?
    } else {
        sqlx::query(
            r#"
            SELECT source_agent_id, target_agent_id, target_name, favorability,
                   last_interaction_tick, synced_at, self_description, description_tick
            FROM agent_relationships
            ORDER BY source_agent_id ASC, synced_at DESC, target_name ASC
            "#,
        )
        .fetch_all(pool)
        .await
        .context("查询 agent_relationships 失败")?
    };

    // 2. 拉取所有相关 key_events（一次查全，内存里分组）
    // 限定到本次涉及到的 source，避免全表扫描。
    let event_rows = if let Some(src) = source_filter {
        sqlx::query(
            r#"
            SELECT source_agent_id, target_agent_id, tick_id, event_type,
                   description, favorability_delta, event_timestamp
            FROM agent_relationship_key_events
            WHERE source_agent_id = $1
            ORDER BY tick_id ASC
            "#,
        )
        .bind(src)
        .fetch_all(pool)
        .await
        .context("查询 agent_relationship_key_events 失败")?
    } else {
        sqlx::query(
            r#"
            SELECT source_agent_id, target_agent_id, tick_id, event_type,
                   description, favorability_delta, event_timestamp
            FROM agent_relationship_key_events
            ORDER BY source_agent_id ASC, tick_id ASC
            "#,
        )
        .fetch_all(pool)
        .await
        .context("查询 agent_relationship_key_events 失败")?
    };

    // 用 (source, target) 分组事件
    use std::collections::HashMap;
    let mut events_by_pair: HashMap<(Uuid, Uuid), Vec<RelationshipKeyEvent>> = HashMap::new();
    for row in &event_rows {
        let key = (
            row.try_get::<Uuid, _>(0)?,
            row.try_get::<Uuid, _>(1)?,
        );
        let ev = RelationshipKeyEvent {
            tick_id: row.try_get(2)?,
            event_type: row.try_get(3)?,
            description: row.try_get(4)?,
            favorability_delta: row.try_get(5)?,
            timestamp: row.try_get(6)?,
        };
        events_by_pair.entry(key).or_default().push(ev);
    }

    // 3. 组装
    let mut result = Vec::with_capacity(rel_rows.len());
    for row in &rel_rows {
        let source_agent_id: Uuid = row.try_get(0)?;
        let target_agent_id: Uuid = row.try_get(1)?;
        let key_events = events_by_pair
            .remove(&(source_agent_id, target_agent_id))
            .unwrap_or_default();
        let mem = RelationshipMemory {
            target_agent_id,
            target_name: row.try_get(2)?,
            favorability: row.try_get(3)?,
            key_events,
            last_interaction_tick: row.try_get(4)?,
            updated_at: row.try_get::<i64, _>(5)?, // synced_at → updated_at（i64 毫秒）
            self_description: row.try_get(6)?,
            description_tick: row.try_get(7)?,
        };
        result.push((source_agent_id, mem));
    }

    Ok(result)
}

/// 查询单个 source agent 的所有关系（含 key_events）
pub async fn get_relationships_by_agent(
    pool: &PgPool,
    source_agent_id: Uuid,
) -> Result<Vec<RelationshipMemory>> {
    Ok(fetch_relationships(pool, Some(source_agent_id))
        .await?
        .into_iter()
        .map(|(_, m)| m)
        .collect())
}

/// 全局查询所有关系（含 key_events）
pub async fn get_all_relationships(
    pool: &PgPool,
) -> Result<Vec<(Uuid, RelationshipMemory)>> {
    fetch_relationships(pool, None).await
}
