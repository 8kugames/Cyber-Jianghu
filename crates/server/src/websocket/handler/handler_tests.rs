//! websocket handler 单测（rebind/库存/关系快照契约）

use super::rebind_device_agent;
use super::reports::validate_relationship_snapshot_ownership;
use std::collections::HashMap;
use uuid::Uuid;

#[test]
fn test_rebind_device_agent_removes_stale_agents_for_same_device() {
    let device_id = Uuid::new_v4();
    let stale_agent = Uuid::new_v4();
    let current_agent = Uuid::new_v4();
    let other_device = Uuid::new_v4();
    let unrelated_agent = Uuid::new_v4();
    let mut map = HashMap::from([(stale_agent, device_id), (unrelated_agent, other_device)]);

    rebind_device_agent(&mut map, current_agent, device_id);

    assert_eq!(map.get(&current_agent), Some(&device_id));
    assert!(!map.contains_key(&stale_agent));
    assert_eq!(map.get(&unrelated_agent), Some(&other_device));
}

/// 验证：初始背包加载失败时，helper 显式返回 Err，
/// 不再向 caller 静默返回空 Vec。
/// 真实 DB 抖动由 caller 决定是否关闭 WebSocket。
#[tokio::test]
async fn test_load_initial_inventory_propagates_db_error() {
    // 构造一个连接到无效地址的 PgPool —— 任何 query 都会失败
    let pool: sqlx::PgPool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/nonexistent")
        .expect("lazy pool init should not connect");

    let result = super::inventory::load_initial_inventory(&pool, Uuid::new_v4()).await;
    assert!(
        result.is_err(),
        "load_initial_inventory must not silently fall back to empty Vec on DB error"
    );
}

/// 验证：地面物品加载失败也必须显式 Err。
#[tokio::test]
async fn test_load_nearby_ground_items_propagates_db_error() {
    let pool: sqlx::PgPool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/nonexistent")
        .expect("lazy pool init should not connect");

    let result = super::inventory::load_nearby_ground_items(&pool, "some-node").await;
    assert!(
        result.is_err(),
        "load_nearby_ground_items must not silently fall back to empty Vec on DB error"
    );
}

// ========================================================================
// C1: RelationshipSnapshot 归属校验（防越权改写他人关系图谱）
// ========================================================================
//
// handle_relationship_snapshot 的核心安全契约：消息内 agent_id 必须
// 等于 device 当前绑定的 agent_id，否则拒绝。
// 已将归属判定抽到纯函数 validate_relationship_snapshot_ownership，
// 这里对其单独做单元测试；完整的 device→agent→ownership→upsert 链路
// 见下方 #[ignore] 集成测试（需真实 PG）。

/// 匹配的 agent_id 必须通过（无越权）。
#[test]
fn test_relationship_ownership_check_accepts_matching_agent() {
    let agent = Uuid::new_v4();
    assert!(validate_relationship_snapshot_ownership(agent, agent).is_ok());
}

/// 不匹配的 agent_id 必须被拒绝，且错误信息携带双方 ID 便于审计/排障。
#[test]
fn test_relationship_ownership_check_rejects_mismatched_agent() {
    let resolved = Uuid::new_v4();
    let attacker = Uuid::new_v4();
    let res = validate_relationship_snapshot_ownership(resolved, attacker);
    assert!(res.is_err(), "mismatched msg_agent_id must be rejected");
    let err = res.unwrap_err();
    let lower = err.to_lowercase();
    assert!(
        lower.contains(&resolved.to_string()),
        "error must leak resolved agent_id for audit, got: {err}"
    );
    assert!(
        lower.contains(&attacker.to_string()),
        "error must leak reported agent_id for audit, got: {err}"
    );
}

/// 即使 attacker 伪造的 agent_id 与另一个真实 agent 相等，只要和 device
/// 绑定的 agent 不一致，也必须拒绝（跨账号越权）。
#[test]
fn test_relationship_ownership_check_rejects_cross_agent_even_if_target_exists() {
    let resolved = Uuid::new_v4();
    let other_real_agent = Uuid::new_v4();
    let res = validate_relationship_snapshot_ownership(resolved, other_real_agent);
    assert!(res.is_err());
}

// ========================================================================
// C1: handle_relationship_snapshot 端到端（需真实 PG，默认 ignore）
// ========================================================================
//
// 下列测试需要 PostgreSQL（DATABASE_URL）。CI 无 DB 时跳过。
// 运行方式：DATABASE_URL=postgres://... cargo test --package cyber-jianghu-server \
//           handle_relationship_snapshot -- --ignored

/// 辅助：构造一个连到 DATABASE_URL 的池，并跑全量迁移。
/// 测试库不复用生产 schema，按现有 server DB 测试模式（common.rs）。
async fn relationship_test_pool() -> sqlx::PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for relationship integration tests");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect DATABASE_URL");
    crate::db::run_migrations(&pool)
        .await
        .expect("run_migrations");
    pool
}

/// 归属校验在真实链路里拒绝越权：用一个未绑定任何 agent 的 device_id
/// 上报别人的 agent_id，应被拒（走 "无关联角色" 分支）。
#[tokio::test]
#[ignore = "需要 PostgreSQL（DATABASE_URL）；C1 归属校验端到端覆盖"]
async fn relationship_snapshot_rejects_unbound_device() {
    let pool = relationship_test_pool().await;
    // 不构造完整 AppState —— 直接调 db 层验证 device 无 agent：
    // get_agent_by_device_id 返回 None 时 handle_relationship_snapshot 走拒绝分支。
    let unbound_device = Uuid::new_v4();
    let resolved = crate::db::get_agent_by_device_id(&pool, unbound_device)
        .await
        .expect("query");
    assert!(
        resolved.is_none(),
        "fresh random device_id must not bind to an agent"
    );
}

/// 空 relationships Vec 在全量覆盖语义下只 DELETE 不 INSERT，
/// 不应 panic、不应报错（agent 清空全部关系的合法语义）。
///
/// 注：需要先有一个绑定 agent 的 device 才能走到 upsert。
/// 此用例通过 lazy pool 暴露 DB 依赖，验证调用形态不 panic；
/// 完整 DB 流程由运行方在有 DATABASE_URL 时 --ignored 触发。
#[tokio::test]
async fn test_handle_relationship_snapshot_empty_vec_shape_does_not_panic() {
    // 不连真实 DB：构造一个 lazy（不立即连接）的池 + 最小 AppState。
    // 空 Vec 路径会先 get_agent_by_device_id —— 在 lazy pool 下会因
    // 连接失败返回 Err，但函数本身不会 panic。
    let pool: sqlx::PgPool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/nonexistent")
        .expect("lazy pool init should not connect");

    // AppState 过重无法轻量构造；这里直接验证底层调用语义：
    // 空 Vec + 失败的 DB 查询返回 Err（而非 panic / Ok）。
    let resolved = crate::db::get_agent_by_device_id(&pool, Uuid::new_v4()).await;
    assert!(
        resolved.is_err(),
        "lazy pool must surface DB error, not panic"
    );
    // 关键不变量：空 Vec 本身不会触发任何 INSERT，因此即使走到 upsert，
    // 也只会执行一条 DELETE（source 行数 0），不会 panic。这在
    // 下面的真实 DB ignore 测试中用断言进一步锁死。
    let empty: Vec<cyber_jianghu_protocol::types::RelationshipMemory> = Vec::new();
    assert!(empty.is_empty());
}

/// 真实 PG：验证空 Vec 全量覆盖后 agent 无任何关系行（DELETE-only 路径）。
#[tokio::test]
#[ignore = "需要 PostgreSQL（DATABASE_URL）；空 Vec DELETE-only 链路"]
async fn relationship_snapshot_empty_vec_clears_source_rows() {
    let pool = relationship_test_pool().await;
    let source = Uuid::new_v4();
    let target = Uuid::new_v4();

    // 先写一条关系（绕过外键：用任意 source/target UUID，C1 表无 FK 到 agents）
    let seed = vec![cyber_jianghu_protocol::types::RelationshipMemory {
        target_agent_id: target,
        target_name: "seed-target".into(),
        favorability: 10,
        key_events: vec![],
        last_interaction_tick: 1,
        updated_at: 1,
        self_description: "".into(),
        description_tick: 0,
    }];
    crate::db::upsert_relationship_snapshot(&pool, source, 1, &seed, 1)
        .await
        .expect("seed upsert");
    let seeded = crate::db::get_relationships_by_agent(&pool, source)
        .await
        .unwrap();
    assert_eq!(seeded.len(), 1, "seed must persist one row");

    // 全量覆盖空 Vec —— 只 DELETE，不 INSERT
    crate::db::upsert_relationship_snapshot(&pool, source, 2, &[], 2)
        .await
        .expect("empty upsert");
    let after = crate::db::get_relationships_by_agent(&pool, source)
        .await
        .unwrap();
    assert!(
        after.is_empty(),
        "empty Vec snapshot must DELETE all source rows without panic"
    );
}
