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

// ============================================================================
// 测试
// ============================================================================
//
// C1 关系图谱存储：upsert_relationship_snapshot 的全量覆盖语义（幂等、替换、
// CHECK 约束）需要真实 PostgreSQL 才能验证 —— 复用 server 既有 DB 测试模式
// （crates/server/src/db/common.rs::test_init_db_pool）：需要真实 PG 的测试
// 用 #[ignore] 标注，通过环境变量 DATABASE_URL 驱动。
//
// 运行：DATABASE_URL=postgres://... cargo test --package cyber-jianghu-server \
//       relationship_ops -- --ignored

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::types::{RelationshipKeyEvent, RelationshipMemory};

    // ------------------------------------------------------------------------
    // 纯静态测试（不需要 DB）—— 锁定 schema 契约
    // ------------------------------------------------------------------------

    /// 验证 C1：迁移 022 必须为 agent_relationships.favorability 设置
    /// CHECK (favorability >= -100 AND favorability <= 100) 约束。
    ///
    /// 这是数据完整性底线：DB 层兜住非法 favorability（如 200），
    /// 即使 agent/protocol 层 BUG 也不会落盘。
    #[test]
    fn test_c1_favorability_check_constraint_present_in_migration() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let migration = manifest_dir
            .join("migrations")
            .join("022_agent_relationships.sql");
        let sql = std::fs::read_to_string(&migration).unwrap_or_else(|e| {
            panic!(
                "C1 迁移缺失：未找到 {}（{}）",
                migration.display(),
                e
            )
        });

        let lower = sql.to_lowercase();
        assert!(
            lower.contains("check (favorability >= -100 and favorability <= 100)"),
            "022 迁移必须包含 favorability CHECK 约束 [-100, 100]，实际内容:\n{sql}"
        );
    }

    /// 验证 C1：迁移 022 必须为 agent_relationships 设置
    /// PRIMARY KEY (source_agent_id, target_agent_id)，这是全量覆盖幂等的基础。
    #[test]
    fn test_c1_relationships_primary_key_on_source_target() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let migration = manifest_dir
            .join("migrations")
            .join("022_agent_relationships.sql");
        let sql = std::fs::read_to_string(&migration).unwrap_or_else(|e| {
            panic!(
                "C1 迁移缺失：未找到 {}（{}）",
                migration.display(),
                e
            )
        });

        let lower = sql.to_lowercase();
        assert!(
            lower.contains("primary key (source_agent_id, target_agent_id)"),
            "022 迁移必须以 (source_agent_id, target_agent_id) 为主键，实际内容:\n{sql}"
        );
    }

    // ------------------------------------------------------------------------
    // 集成测试（需要真实 PostgreSQL）—— 用 #[ignore] 标注
    // ------------------------------------------------------------------------

    /// 测试辅助：连到 DATABASE_URL 并跑全量迁移（对齐 common.rs 既有模式）。
    async fn relationship_test_pool() -> PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for relationship integration tests");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect DATABASE_URL");
        crate::db::run_migrations(&pool).await.expect("run_migrations");
        pool
    }

    /// 构造一条关系记忆（带若干 key_events）。
    fn make_rel(
        target: Uuid,
        target_name: &str,
        favorability: i32,
        events: Vec<RelationshipKeyEvent>,
    ) -> RelationshipMemory {
        RelationshipMemory {
            target_agent_id: target,
            target_name: target_name.into(),
            favorability,
            key_events: events,
            last_interaction_tick: 100,
            updated_at: 1000,
            self_description: "d".into(),
            description_tick: 50,
        }
    }

    fn make_event(tick: i64, delta: i32) -> RelationshipKeyEvent {
        RelationshipKeyEvent {
            tick_id: tick,
            event_type: "trade".into(),
            description: format!("event@{tick}"),
            favorability_delta: delta,
            timestamp: tick * 1000,
        }
    }

    /// 清理测试残留（避免测试间污染）。
    async fn cleanup_source(pool: &PgPool, source: Uuid) {
        // CASCADE 会带走 key_events；显式 DELETE 防御
        let _ = sqlx::query("DELETE FROM agent_relationships WHERE source_agent_id = $1")
            .bind(source)
            .execute(pool)
            .await;
    }

    /// C1 幂等性：同一 source 同一快照调用 upsert 两次，行数不翻倍
    /// （关系数不变、key_events 数不变）。验证 DELETE-then-INSERT 语义。
    #[tokio::test]
    #[ignore = "需要 PostgreSQL（DATABASE_URL）；C1 幂等性集成测试"]
    async fn test_upsert_snapshot_idempotent() {
        let pool = relationship_test_pool().await;
        let source = Uuid::new_v4();
        cleanup_source(&pool, source).await;

        let target_a = Uuid::new_v4();
        let target_b = Uuid::new_v4();
        let snapshot = vec![
            make_rel(target_a, "A", 10, vec![make_event(1, 5), make_event(2, 5)]),
            make_rel(
                target_b,
                "B",
                -10,
                vec![make_event(3, -5)],
            ),
        ];

        // 第一次写入
        upsert_relationship_snapshot(&pool, source, 1, &snapshot, 1_000)
            .await
            .expect("first upsert");
        let after_first = get_relationships_by_agent(&pool, source)
            .await
            .expect("fetch after first");
        assert_eq!(after_first.len(), 2, "first upsert should persist 2 relations");
        let events_first: usize = after_first.iter().map(|r| r.key_events.len()).sum();
        assert_eq!(events_first, 3, "first upsert should persist 3 key_events");

        // 第二次完全相同的快照 —— 必须幂等
        upsert_relationship_snapshot(&pool, source, 1, &snapshot, 2_000)
            .await
            .expect("second upsert");
        let after_second = get_relationships_by_agent(&pool, source)
            .await
            .expect("fetch after second");
        assert_eq!(
            after_second.len(),
            2,
            "idempotent upsert must NOT duplicate relation rows"
        );
        let events_second: usize = after_second.iter().map(|r| r.key_events.len()).sum();
        assert_eq!(
            events_second, 3,
            "idempotent upsert must NOT duplicate key_event rows"
        );

        cleanup_source(&pool, source).await;
    }

    /// C1 全量覆盖语义：先 upsert [A→B, A→C]，再 upsert [A→B]（只剩一条），
    /// 验证 A→C 被删除（全量覆盖，而非增量合并）。
    #[tokio::test]
    #[ignore = "需要 PostgreSQL（DATABASE_URL）；C1 全量覆盖语义集成测试"]
    async fn test_upsert_snapshot_replaces_full() {
        let pool = relationship_test_pool().await;
        let source = Uuid::new_v4();
        cleanup_source(&pool, source).await;

        let target_b = Uuid::new_v4();
        let target_c = Uuid::new_v4();

        // 第一次：两条关系
        let first = vec![
            make_rel(target_b, "B", 20, vec![make_event(1, 10)]),
            make_rel(target_c, "C", -5, vec![make_event(2, -5)]),
        ];
        upsert_relationship_snapshot(&pool, source, 1, &first, 1_000)
            .await
            .expect("first upsert");
        let after_first = get_relationships_by_agent(&pool, source)
            .await
            .expect("fetch after first");
        assert_eq!(after_first.len(), 2);

        // 第二次：只剩 [A→B]（A→C 应被全量覆盖删除）
        let second = vec![make_rel(target_b, "B", 30, vec![make_event(1, 10)])];
        upsert_relationship_snapshot(&pool, source, 2, &second, 2_000)
            .await
            .expect("second upsert");
        let after_second = get_relationships_by_agent(&pool, source)
            .await
            .expect("fetch after second");

        assert_eq!(
            after_second.len(),
            1,
            "full-replace semantics: source must only retain the 2nd snapshot's relations"
        );
        // 剩下那条必须是 target_b（被覆盖更新，favorability=30）
        let only = &after_second[0];
        assert_eq!(only.target_agent_id, target_b);
        assert_eq!(only.favorability, 30, "remaining relation must reflect new snapshot values");
        // A→C 必须不再出现
        assert!(
            !after_second.iter().any(|r| r.target_agent_id == target_c),
            "full-replace must delete relations absent from new snapshot (A→C)"
        );

        cleanup_source(&pool, source).await;
    }

    /// C1 CHECK 约束：favorability=200 必须被 DB 层拒绝（CHECK [-100, 100]）。
    /// 锁定 schema 兜底，避免 agent/protocol 层 BUG 写入非法值。
    #[tokio::test]
    #[ignore = "需要 PostgreSQL（DATABASE_URL）；C1 favorability CHECK 约束测试"]
    async fn test_favorability_check_constraint() {
        let pool = relationship_test_pool().await;
        let source = Uuid::new_v4();
        cleanup_source(&pool, source).await;

        let target = Uuid::new_v4();
        // 200 越界 —— DB CHECK 必须拒绝整个事务
        let bad = vec![make_rel(target, "bad", 200, vec![])];
        let result = upsert_relationship_snapshot(&pool, source, 1, &bad, 1_000).await;

        assert!(
            result.is_err(),
            "favorability=200 must violate CHECK constraint and be rejected"
        );

        // 事务回滚 —— 不应残留任何行
        let after = get_relationships_by_agent(&pool, source)
            .await
            .expect("fetch after rejected upsert");
        assert!(
            after.is_empty(),
            "rejected upsert must not leave partial rows (transaction rollback)"
        );

        // 合法边界值必须通过：favorability = 100（上界）与 -100（下界）
        let valid = vec![
            make_rel(Uuid::new_v4(), "hi", 100, vec![]),
            make_rel(Uuid::new_v4(), "lo", -100, vec![]),
        ];
        upsert_relationship_snapshot(&pool, source, 1, &valid, 2_000)
            .await
            .expect("boundary favorability ±100 must be accepted");
        let ok_rows = get_relationships_by_agent(&pool, source)
            .await
            .expect("fetch valid");
        assert_eq!(ok_rows.len(), 2);

        cleanup_source(&pool, source).await;
    }

    /// C1 空 Vec：只 DELETE 不 INSERT，不 panic，清空 source 全部关系。
    #[tokio::test]
    #[ignore = "需要 PostgreSQL（DATABASE_URL）；C1 空 Vec DELETE-only 路径"]
    async fn test_upsert_snapshot_empty_vec_clears_all() {
        let pool = relationship_test_pool().await;
        let source = Uuid::new_v4();
        cleanup_source(&pool, source).await;

        // 先 seed 两条
        let seed = vec![
            make_rel(Uuid::new_v4(), "X", 5, vec![make_event(1, 1)]),
            make_rel(Uuid::new_v4(), "Y", 5, vec![make_event(2, 1)]),
        ];
        upsert_relationship_snapshot(&pool, source, 1, &seed, 1_000)
            .await
            .expect("seed");
        assert_eq!(
            get_relationships_by_agent(&pool, source).await.unwrap().len(),
            2
        );

        // 空 Vec —— DELETE-only
        upsert_relationship_snapshot(&pool, source, 2, &[], 2_000)
            .await
            .expect("empty Vec upsert must not error");
        let after = get_relationships_by_agent(&pool, source).await.unwrap();
        assert!(
            after.is_empty(),
            "empty Vec snapshot must DELETE all source relations without panic"
        );

        cleanup_source(&pool, source).await;
    }

    /// C1 key_events 分组组装：fetch 必须把事件正确归到各自 (source, target) 对。
    /// 锁定 fetch_relationships 的 HashMap 分组逻辑。
    #[tokio::test]
    #[ignore = "需要 PostgreSQL（DATABASE_URL）；C1 key_events 分组组装测试"]
    async fn test_get_relationships_groups_key_events_by_pair() {
        let pool = relationship_test_pool().await;
        let source = Uuid::new_v4();
        cleanup_source(&pool, source).await;

        let target_a = Uuid::new_v4();
        let target_b = Uuid::new_v4();
        let snapshot = vec![
            make_rel(target_a, "A", 10, vec![make_event(1, 1), make_event(3, 2)]),
            make_rel(target_b, "B", 20, vec![make_event(2, 5)]),
        ];
        upsert_relationship_snapshot(&pool, source, 1, &snapshot, 1_000)
            .await
            .expect("upsert");

        let rows = get_relationships_by_agent(&pool, source).await.unwrap();
        assert_eq!(rows.len(), 2);
        for rel in &rows {
            match rel.target_agent_id {
                t if t == target_a => {
                    assert_eq!(rel.key_events.len(), 2, "target_a must have 2 events");
                    // 按 tick_id ASC 排序（与 fetch_relationships 契约一致）
                    let ticks: Vec<i64> = rel.key_events.iter().map(|e| e.tick_id).collect();
                    assert_eq!(ticks, vec![1, 3], "key_events must be tick_id ASC");
                }
                t if t == target_b => {
                    assert_eq!(rel.key_events.len(), 1, "target_b must have 1 event");
                }
                _ => panic!("unexpected target"),
            }
        }

        cleanup_source(&pool, source).await;
    }
}
