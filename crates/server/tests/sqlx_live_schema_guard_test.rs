//! sqlx 活库守卫测试（#[ignore] 默认跳过）
//!
//! 防的是一族"编译期/单测期完全不可见、只在真实 PostgreSQL 上执行才暴露"的 SQL 缺陷：
//!
//! 1. **聚合类型提升**：PostgreSQL 的 `COUNT(*)`/`SUM(int4)` 返回 BIGINT，
//!    `EXTRACT(EPOCH)`/`AVG`/`PERCENTILE_CONT` 返回 numeric——sqlx 按目标 Rust
//!    类型严格解码，错配即运行期 mismatched types；若又被 `.ok()`/`unwrap_or`
//!    吞掉，就静默变成 None/0（validator 吃馒头、telemetry survival_time
//!    两次线上事故均属此类）。
//! 2. **schema 漂移**：SQL 引用不存在的表/列（如 game_rules_config 表、
//!    server_deployment.deployment_time 列），语句在 Parse 期即失败。
//!
//! 空表路径测不出解码缺陷（解码根本不执行），因此本测试必须写入夹具数据。
//! 新增/修改聚合或含计算列的 SQL 时，应在此补对应执行路径。
//!
//! 运行（一次性可丢弃库，勿指向生产/开发库）：
//!
//! ```bash
//! docker exec cyber-jianghu-postgres createdb -U postgres cyber_jianghu_verify
//! # 密码见 crates/server/docker-compose.yml 的 POSTGRES_PASSWORD
//! DATABASE_URL=postgres://postgres:<POSTGRES_PASSWORD>@localhost:5432/cyber_jianghu_verify \
//!   cargo nextest run -p cyber-jianghu-server --test sqlx_live_schema_guard_test \
//!   --run-ignored=only --nocapture
//! ```
//!
//! 未设置 DATABASE_URL 时自动跳过，不阻塞 CI。数据安全：夹具使用每次运行
//! 新生成的随机 UUID，清理只针对这些 id；telemetry 聚合行按「本测试开始
//! 之后落库」精确删除。

use sqlx::PgPool;
use uuid::Uuid;

use cyber_jianghu_server::actions::get_inventory_item_quantity;
use cyber_jianghu_server::telemetry::collector::collect_from_agents;
use cyber_jianghu_server::telemetry::storage::query_aggregations;

/// 读取 DATABASE_URL；不存在则 skip
fn test_db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

async fn test_pool(url: &str) -> PgPool {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(url)
        .await
        .expect("connect DATABASE_URL");
    cyber_jianghu_server::db::run_migrations(&pool)
        .await
        .expect("run_migrations");
    pool
}

/// 建一个最小可用角色（agents 表外键依赖 devices）
async fn seed_agent(pool: &PgPool, name: &str) -> Uuid {
    let device_id = Uuid::new_v4();
    sqlx::query("INSERT INTO devices (device_id, auth_token) VALUES ($1, $2)")
        .bind(device_id)
        .bind(format!("guard-test-token-{device_id}"))
        .execute(pool)
        .await
        .expect("insert device");

    let agent_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agents (agent_id, device_id, name, system_prompt, status) \
         VALUES ($1, $2, $3, 'test', 'active')",
    )
    .bind(agent_id)
    .bind(device_id)
    .bind(name)
    .execute(pool)
    .await
    .expect("insert agent");
    agent_id
}

/// 清理本次运行写入的全部数据（只针对随机 UUID；agent_inventory 随 agent 级联删除）
async fn cleanup(pool: &PgPool, agent_ids: &[Uuid]) {
    for agent_id in agent_ids {
        let device_id: Option<Uuid> =
            sqlx::query_scalar("DELETE FROM agents WHERE agent_id = $1 RETURNING device_id")
                .bind(agent_id)
                .fetch_optional(pool)
                .await
                .expect("cleanup agent");
        if let Some(device_id) = device_id {
            sqlx::query("DELETE FROM devices WHERE device_id = $1")
                .bind(device_id)
                .execute(pool)
                .await
                .expect("cleanup device");
        }
    }
}

/// 背包持有量 SUM 解码：SUM(INTEGER) 返回 BIGINT，旧实现以 i32 解码必失败
/// （曾被误报为「服务端存储异常」，导致吃/喝/用全量被拒）
#[tokio::test]
#[ignore = "需要真实 PostgreSQL（DATABASE_URL）；见文件头说明"]
async fn inventory_quantity_sum_decodes_bigint() {
    let Some(url) = test_db_url() else {
        eprintln!("跳过: DATABASE_URL 未设置");
        return;
    };
    let pool = test_pool(&url).await;
    let agent_id = seed_agent(&pool, "解码守卫甲").await;
    let item_id = format!("guard-{}", Uuid::new_v4().simple());

    sqlx::query("INSERT INTO items (item_id, name, item_type) VALUES ($1, '守卫物品', 'material')")
        .bind(&item_id)
        .execute(&pool)
        .await
        .expect("insert item");

    sqlx::query("INSERT INTO agent_inventory (agent_id, item_id, quantity) VALUES ($1, $2, 5)")
        .bind(agent_id)
        .bind(&item_id)
        .execute(&pool)
        .await
        .expect("insert inventory");

    let qty = get_inventory_item_quantity(&pool, agent_id, &item_id)
        .await
        .expect("SUM 解码为 i64 应成功");
    assert_eq!(qty, 5);

    // COALESCE 空集分支同样返回 BIGINT 0
    let absent = get_inventory_item_quantity(&pool, agent_id, "guard-absent")
        .await
        .expect("空集解码");
    assert_eq!(absent, 0);

    cleanup(&pool, &[agent_id]).await;
    sqlx::query("DELETE FROM items WHERE item_id = $1")
        .bind(&item_id)
        .execute(&pool)
        .await
        .expect("cleanup item");
}

/// survival_time 聚合：EXTRACT(EPOCH)→numeric 须经 ::float8 才能按 f64 解码；
/// 且引用的表/列必须真实存在（server_deployment.deployed_at）。
/// 旧实现两处缺陷：列名漂移使每轮必失败、numeric 解码被 .ok() 吞成 None。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL（DATABASE_URL）；见文件头说明"]
async fn survival_time_aggregation_decodes_numeric_as_f64() {
    let Some(url) = test_db_url() else {
        eprintln!("跳过: DATABASE_URL 未设置");
        return;
    };
    let pool = test_pool(&url).await;
    let test_started = chrono::Utc::now();

    // 夹具：本周期内死亡的角色。birth_tick=60、rspt=120 → duration ≈ 7200s+，
    // 与 deployed_at 的相对漂移无关（verify 库刚迁移完，deployed_at ≈ now）
    let agent_id = seed_agent(&pool, "解码守卫乙").await;
    sqlx::query(
        "UPDATE agents SET status = 'dead', birth_tick = 60, retired_at = NOW() \
         WHERE agent_id = $1",
    )
    .bind(agent_id)
    .execute(&pool)
    .await
    .expect("mark dead");

    collect_from_agents(&pool, "survival_time", &[], &[], 60, 120.0)
        .await
        .expect("survival_time 聚合应执行成功");

    let rows = query_aggregations(&pool, "survival_time", 1, 0)
        .await
        .expect("读回聚合结果");
    let row = rows.first().expect("应写入一条 survival_time 聚合");
    for key in [
        "avg_duration_seconds",
        "p50_duration_seconds",
        "p95_duration_seconds",
    ] {
        let v = row
            .metrics
            .get(key)
            .and_then(|v| v.as_f64())
            .unwrap_or_else(|| panic!("{key} 应为数值（numeric 解码缺陷会静默丢失该字段）"));
        assert!(v > 7000.0, "{key} 应约为 birth_tick*rspt=7200s，实测 {v}");
    }

    // 清理：删除本次写入的聚合行（period_end 落在测试开始之后）与夹具
    sqlx::query(
        "DELETE FROM telemetry_aggregations \
         WHERE aggregation_name = 'survival_time' AND period_end >= $1",
    )
    .bind(test_started)
    .execute(&pool)
    .await
    .expect("cleanup aggregations");
    cleanup(&pool, &[agent_id]).await;
}
