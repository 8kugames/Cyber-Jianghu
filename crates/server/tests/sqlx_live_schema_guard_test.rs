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
use cyber_jianghu_server::telemetry::collector::{collect_from_agents, run_aggregation};
use cyber_jianghu_server::telemetry::storage::query_aggregations;

/// 迁移段串行化的 advisory lock key（任意固定魔数，仅本测试文件使用）
const GUARD_MIGRATION_LOCK_KEY: i64 = 715_573;

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
    // run_migrations 是无版本表/无锁的幂等 DDL 重放，多个测试进程并行重放会
    // 触发 DDL 竞态（CREATE INDEX/COMMENT 同名冲突）。在独占连接上持会话级
    // advisory lock 串行化迁移段：其他进程的 lock 请求阻塞直至本段完成，
    // 锁随连接归还自动释放，任意并行度下安全。
    let mut conn = pool.acquire().await.expect("acquire guard conn");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(GUARD_MIGRATION_LOCK_KEY)
        .execute(&mut *conn)
        .await
        .expect("advisory lock");
    cyber_jianghu_server::db::run_migrations(&pool)
        .await
        .expect("run_migrations");
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(GUARD_MIGRATION_LOCK_KEY)
        .execute(&mut *conn)
        .await
        .expect("advisory unlock");
    drop(conn);
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

/// interaction_activity 的 partner 分支：SQL 由 jsonb_partner_fields 配置
/// format! 拼接（宏不可用），聚合列 COUNT(*)/COUNT(DISTINCT) 的解码只能靠
/// 活库夹具验证——空窗口不触发解码，必须写入带 partner 字段的动作日志。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL（DATABASE_URL）；见文件头说明"]
async fn interaction_activity_partner_branch_decodes_counts() {
    let Some(url) = test_db_url() else {
        eprintln!("跳过: DATABASE_URL 未设置");
        return;
    };
    let pool = test_pool(&url).await;
    let test_started = chrono::Utc::now();

    let agent_id = seed_agent(&pool, "解码守卫丙").await;
    sqlx::query(
        "INSERT INTO agent_action_logs (tick_id, agent_id, action_type, result, action_data) \
         VALUES ($1, $2, '说话', 'success', $3::jsonb)",
    )
    .bind(9_100_000_000i64)
    .bind(agent_id)
    .bind(format!("{{\"recipient_id\": \"{}\"}}", Uuid::new_v4()))
    .execute(&pool)
    .await
    .expect("insert action log");

    run_aggregation(
        &pool,
        "interaction_activity",
        "agent_action_logs",
        &[],
        &[],
        &["recipient_id".to_string()],
        60,
    )
    .await
    .expect("interaction_activity 聚合应执行成功");

    let rows = query_aggregations(&pool, "interaction_activity", 1, 0)
        .await
        .expect("读回聚合结果");
    let row = rows.first().expect("应写入一条 interaction_activity 聚合");
    let action_count = row
        .metrics
        .get("action_count")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| panic!("action_count 应为数值（解码失败会缺字段）"));
    let unique = row
        .metrics
        .get("unique_interacting_agents")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| panic!("unique_interacting_agents 应为数值"));
    assert_eq!(action_count, 1, "带 recipient_id 的动作应被计数");
    assert_eq!(unique, 1);

    sqlx::query(
        "DELETE FROM telemetry_aggregations \
         WHERE aggregation_name = 'interaction_activity' AND period_end >= $1",
    )
    .bind(test_started)
    .execute(&pool)
    .await
    .expect("cleanup aggregations");
    sqlx::query("DELETE FROM agent_action_logs WHERE agent_id = $1")
        .bind(agent_id)
        .execute(&pool)
        .await
        .expect("cleanup action logs");
    cleanup(&pool, &[agent_id]).await;
}

/// location_traffic：select_clause 由 metrics 配置拼接（宏不可用），
/// COUNT(DISTINCT agent_id)/COUNT(*) 聚合列解码靠活库夹具验证。
#[tokio::test]
#[ignore = "需要真实 PostgreSQL（DATABASE_URL）；见文件头说明"]
async fn location_traffic_dynamic_select_decodes_counts() {
    let Some(url) = test_db_url() else {
        eprintln!("跳过: DATABASE_URL 未设置");
        return;
    };
    let pool = test_pool(&url).await;
    let test_started = chrono::Utc::now();

    let agent_id = seed_agent(&pool, "解码守卫丁").await;
    sqlx::query(
        "INSERT INTO agent_states (agent_id, tick_id, node_id) VALUES ($1, $2, 'guard-node')",
    )
    .bind(agent_id)
    .bind(9_100_000_000i64)
    .execute(&pool)
    .await
    .expect("insert agent state");

    run_aggregation(
        &pool,
        "location_traffic",
        "agent_states",
        &["node_id".to_string()],
        &["agent_count".to_string(), "state_count".to_string()],
        &[],
        60,
    )
    .await
    .expect("location_traffic 聚合应执行成功");

    let rows = query_aggregations(&pool, "location_traffic", 5, 0)
        .await
        .expect("读回聚合结果");
    let row = rows
        .iter()
        .find(|r| r.group_by_value.as_deref() == Some("guard-node"))
        .expect("应写入 guard-node 的 location_traffic 聚合");
    let agent_count = row
        .metrics
        .get("agent_count")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| panic!("agent_count 应为数值（解码失败会缺字段）"));
    let state_count = row
        .metrics
        .get("state_count")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| panic!("state_count 应为数值"));
    assert_eq!(agent_count, 1);
    assert_eq!(state_count, 1);

    sqlx::query(
        "DELETE FROM telemetry_aggregations \
         WHERE aggregation_name = 'location_traffic' AND period_end >= $1",
    )
    .bind(test_started)
    .execute(&pool)
    .await
    .expect("cleanup aggregations");
    sqlx::query("DELETE FROM agent_states WHERE agent_id = $1")
        .bind(agent_id)
        .execute(&pool)
        .await
        .expect("cleanup agent states");
    cleanup(&pool, &[agent_id]).await;
}

// ============================================================================
// 治理管道 reopen 断链守卫（2026-09-14 审计 M2/M3 修复的回归锁定）
//
// M2：admin 关单必须置 stage='done'——upsert_proposal_group 的 reopen CASE
//     仅认 stage='done'，否则同类新提议追加进已关闭 group 永不重审。
// M3：get_group_proposal_ids 必须 DESC——引擎按 proposal_ids.first() 取样
//     代表提案，ASC 会重审上一轮已审过的最旧提案而非触发重开的新证据。
// ============================================================================
#[tokio::test]
#[ignore = "需要真实 PostgreSQL（DATABASE_URL）；见文件头说明"]
async fn governance_closed_group_reopens_and_samples_latest() {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        let pool = test_pool(&url).await;
        let agent_id = seed_agent(&pool, "governance-guard-agent").await;
        let group_id = uuid::Uuid::new_v4();
        let pid_old = uuid::Uuid::new_v4();
        let pid_new = uuid::Uuid::new_v4();

        // 夹具：已关闭的 group（status=rejected, stage=done）+ 新旧两条提案
        sqlx::query(
            "INSERT INTO action_evolution_proposal_groups \
             (id, similarity_key, primary_soul, status, stage, proposal_ids) \
             VALUES ($1, $2, 'fuxi', 'rejected', 'done', $3::jsonb)",
        )
        .bind(group_id)
        .bind(format!("guard-{}", uuid::Uuid::new_v4()))
        .bind(serde_json::json!([pid_old]).to_string())
        .execute(&pool)
        .await
        .expect("insert closed group");

        for (pid, tick) in [(pid_old, 100), (pid_new, 200)] {
            sqlx::query(
                "INSERT INTO action_evolution_proposals \
                 (id, agent_id, tick_id, proposed_action_type, rationale) \
                 VALUES ($1, $2, $3, '测试动作', '守卫夹具：治理 reopen 断链')",
            )
            .bind(pid)
            .bind(agent_id)
            .bind(tick)
            .execute(&pool)
            .await
            .expect("insert proposal");
            sqlx::query(
                "INSERT INTO action_evolution_group_proposals \
                 (proposal_group_id, proposal_id) VALUES ($1, $2)",
            )
            .bind(group_id)
            .bind(pid)
            .execute(&pool)
            .await
            .expect("insert link");
        }
        // 制造 created_at 差异：pid_new 更晚入库（同秒插入时 DESC 稳定序靠次键）
        sqlx::query(
            "UPDATE action_evolution_proposals SET created_at = NOW() - interval '2 hours' \
             WHERE id = $1",
        )
        .bind(pid_old)
        .execute(&pool)
        .await
        .expect("backdate old proposal");

        // M3 守卫：proposal_store::get_group_proposal_ids 的 DESC 序（SQL 与实现同文），
        // 引擎按 proposal_ids.first() 取样——首元素必须是最新提案
        let ids = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT proposal_id \
             FROM action_evolution_group_proposals \
             WHERE proposal_group_id = $1 \
             ORDER BY created_at DESC, proposal_id DESC",
        )
        .bind(group_id)
        .fetch_all(&pool)
        .await
        .expect("query ids desc");
        assert_eq!(
            ids.first(),
            Some(&pid_new),
            "M3：取样必须取最新提案（触发重开的新证据），而非最旧"
        );

        // M2 守卫：关单写入 stage='done' 后，upsert 的 reopen CASE 必须识别它
        //（CASE 表达式与 proposal_store::upsert_proposal_group 同文）
        let reopened: (String, String) = sqlx::query_as(
            "SELECT \
               CASE WHEN stage = 'done' THEN 'pending_review' ELSE status END, \
               CASE WHEN stage = 'done' THEN 'awaiting_fuxi_initial' ELSE stage END \
             FROM action_evolution_proposal_groups WHERE id = $1",
        )
        .bind(group_id)
        .fetch_one(&pool)
        .await
        .expect("reopen case eval");
        assert_eq!(
            reopened,
            ("pending_review".into(), "awaiting_fuxi_initial".into()),
            "M2：stage='done' 必须被 reopen CASE 识别（管道重启）"
        );

        // 清理（links/groups 级联；proposals 随 agents 级联）
        sqlx::query("DELETE FROM action_evolution_proposal_groups WHERE id = $1")
            .bind(group_id)
            .execute(&pool)
            .await
            .expect("cleanup groups");
        cleanup(&pool, &[agent_id]).await;
        pool.close().await;
    }
}

// ============================================================================
// 资源存量守卫（阶段 2：采集 Saga 扣减的 stock >= quantity 守卫与日再生回补）
// ============================================================================
#[tokio::test]
#[ignore = "需要真实 PostgreSQL（DATABASE_URL）；见文件头说明"]
async fn resource_stock_consume_guard_and_regen() {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        let pool = test_pool(&url).await;
        let node = format!("guard-node-{}", uuid::Uuid::new_v4());
        // 一次性库全量清场：日再生是全表 UPDATE，残留行会干扰 rows_affected 断言
        sqlx::query("DELETE FROM resource_nodes")
            .execute(&pool)
            .await
            .expect("clear resource_nodes");

        sqlx::query(
            "INSERT INTO resource_nodes (node_id, item_id, stock, max_stock, regen_per_game_day) \
             VALUES ($1, '草药', 3, 10, 4)",
        )
        .bind(&node)
        .execute(&pool)
        .await
        .expect("insert resource node");

        // 扣 2：成功
        let n = sqlx::query(
            "UPDATE resource_nodes SET stock = stock - $3, updated_at = NOW() \
             WHERE node_id = $1 AND item_id = $2 AND stock >= $3",
        )
        .bind(&node)
        .bind("草药")
        .bind(2)
        .execute(&pool)
        .await
        .expect("consume 2")
        .rows_affected();
        assert_eq!(n, 1);

        // 扣 2：守卫拦截（余 1 不足）
        let n = sqlx::query(
            "UPDATE resource_nodes SET stock = stock - $3 \
             WHERE node_id = $1 AND item_id = $2 AND stock >= $3",
        )
        .bind(&node)
        .bind("草药")
        .bind(2)
        .execute(&pool)
        .await
        .expect("consume 2 again")
        .rows_affected();
        assert_eq!(n, 0, "stock >= quantity 守卫必须拦截超扣");

        // 日再生：+4 受加成后回补，上限 max_stock=10（LEAST 截断）
        let n = sqlx::query(
            "UPDATE resource_nodes \
             SET stock = LEAST(stock + CEIL(regen_per_game_day * $1)::bigint, max_stock), \
                 updated_at = NOW() \
             WHERE regen_per_game_day > 0 AND stock < max_stock",
        )
        .bind(1.5f64)
        .execute(&pool)
        .await
        .expect("regen")
        .rows_affected();
        assert!(n >= 1, "本夹具行必须被回补（其他残留行回补不碍守卫语义）");

        let (stock, max_stock): (i64, i64) = sqlx::query_as(
            "SELECT stock, max_stock FROM resource_nodes WHERE node_id = $1 AND item_id = '草药'",
        )
        .bind(&node)
        .fetch_one(&pool)
        .await
        .expect("read back");
        assert_eq!(
            stock, 7,
            "1 + CEIL(4*1.5)=6 → 7（未触及 max_stock=10 截断）"
        );
        assert_eq!(max_stock, 10);

        sqlx::query("DELETE FROM resource_nodes WHERE node_id = $1")
            .bind(&node)
            .execute(&pool)
            .await
            .expect("cleanup");
        pool.close().await;
    }
}
