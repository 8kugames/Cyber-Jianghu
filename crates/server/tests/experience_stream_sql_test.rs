//! 经历日志流水分页 SQL 的活库集成测试（#[ignore] 默认跳过）
//!
//! 这两条语句（计数 + 取键）共用 `EXPERIENCE_ROW_FILTERS` 片段，占位符编号
//! 必须与 `.bind()` 顺序逐位对应。sqlx 会把绑定值的 Rust 类型作为参数类型
//! 发给 Postgres，一旦错位，语句在 Parse 阶段就失败——编译、clippy 与全部
//! 单测都不会发现，只有真的执行一次查询才暴露。本测试补的正是这道缺口。
//!
//! 同时承担 AGENTS.md「SQL / sqlx 约定」要求的活库守卫：`COUNT(*)` 返回
//! BIGINT，`count_experience_ticks` 以 i64 解码，本测试写入非空夹具后执行
//! 该聚合（空表路径测不出解码缺陷）。与同目录的
//! `sqlx_live_schema_guard_test.rs` 是互补而非重复关系：那份是跨切面的
//! schema/类型守卫（一处失败说明某类 SQL 普遍有问题），本文件是端点级守卫
//! （失败即定位到经历日志这个端点的具体语句），失败定位粒度不同，故各自保留。
//!
//! 需要一个可丢弃的库（迁移会被执行，测试会写入并清理自己的数据）：
//!
//! ```bash
//! docker exec cyber-jianghu-postgres createdb -U postgres cyber_jianghu_verify
//! DATABASE_URL=postgres://postgres:<POSTGRES_PASSWORD>@localhost:5432/cyber_jianghu_verify \
//!   cargo nextest run -p cyber-jianghu-server --test experience_stream_sql_test \
//!   --run-ignored=only --nocapture
//! ```
//!
//! 连接串中的口令见 docker-compose.yml 的 POSTGRES_PASSWORD，不在此硬编码。
//! 未设置 DATABASE_URL 时自动跳过（返回 Ok），不阻塞 CI。注意本测试带
//! `#[ignore]`，常规 `cargo nextest run --workspace` 不会执行它；CI 目前也
//! 不带 `--run-ignored`，故这道守卫实际只在本地/发布前按上面命令执行。
//!
//! 数据安全：写入的 agent_id 是每次运行新生成的随机 UUID，清理（`cleanup`）
//! 只针对这些 id；断言也全部按自己的 agent_id（或自己的 tick 区间）过滤，
//! 因此即便误指向生产库，也只可能新增并删除自己那几行，不会碰到既有数据。
//! 测试之间可并发/重复执行，互不干扰。

use sqlx::PgPool;
use uuid::Uuid;

/// 本测试使用的 tick 基准值
///
/// 真实 tick_id 由墙钟推导（2026 年约 1.3e7），取 9e9 量级只为让本测试的卡片
/// 稳定排在分页结果最前面，**不构成排他保留区**：断言一律按本测试自己的
/// agent_id 过滤，所以即使该区间内存在他人夹具也不会误判，更不会去删它。
const TEST_TICK_BASE: i64 = 9_000_000_000;

// 模块本身是私有的，条目由 `handlers::dashboard` 的 glob 再导出
use cyber_jianghu_server::handlers::dashboard::{
    ExperienceStreamFilters, count_experience_ticks, fetch_experience_tick_keys,
};

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

/// 建一个最小可用角色（agent_action_logs.agent_id 有外键约束）
async fn seed_agent(pool: &PgPool, name: &str) -> Uuid {
    let device_id = Uuid::new_v4();
    sqlx::query("INSERT INTO devices (device_id, auth_token) VALUES ($1, $2)")
        .bind(device_id)
        .bind(format!("exp-test-token-{device_id}"))
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

/// 写一条动作日志行并返回该行 id
async fn seed_action(
    pool: &PgPool,
    agent_id: Uuid,
    tick_id: i64,
    pipe_seq: i32,
    action_type: &str,
    result: &str,
) {
    sqlx::query(
        "INSERT INTO agent_action_logs \
         (tick_id, agent_id, pipe_seq, action_type, result, created_at) \
         VALUES ($1, $2, $3, $4, $5, CURRENT_TIMESTAMP)",
    )
    .bind(tick_id)
    .bind(agent_id)
    .bind(pipe_seq)
    .bind(action_type)
    .bind(result)
    .execute(pool)
    .await
    .expect("insert action log");
}

/// 写一条位置快照（位置筛选走 LATERAL，取不晚于该 tick 的最近一条）
async fn seed_state(pool: &PgPool, agent_id: Uuid, tick_id: i64, node_id: &str) {
    sqlx::query(
        "INSERT INTO agent_states (agent_id, tick_id, attributes, node_id, is_alive) \
         VALUES ($1, $2, '{}', $3, true) \
         ON CONFLICT (agent_id, tick_id) DO UPDATE SET node_id = EXCLUDED.node_id",
    )
    .bind(agent_id)
    .bind(tick_id)
    .bind(node_id)
    .execute(pool)
    .await
    .expect("insert agent state");
}

/// 清理本次运行写入的全部数据（只针对随机 UUID，不会命中既有数据）
async fn cleanup(pool: &PgPool, agent_ids: &[Uuid]) {
    for agent_id in agent_ids {
        sqlx::query("DELETE FROM agent_action_logs WHERE agent_id = $1")
            .bind(agent_id)
            .execute(pool)
            .await
            .expect("cleanup action logs");
        sqlx::query("DELETE FROM agent_states WHERE agent_id = $1")
            .bind(agent_id)
            .execute(pool)
            .await
            .expect("cleanup agent states");
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

/// 两条语句必须能 Prepare 并返回同一套 tick 卡片键
#[tokio::test]
#[ignore = "需要真实 PostgreSQL（DATABASE_URL）；见文件头说明"]
async fn experience_stream_queries_prepare_and_group_by_tick() {
    let Some(url) = test_db_url() else {
        eprintln!("跳过: DATABASE_URL 未设置");
        return;
    };
    let pool = test_pool(&url).await;

    let agent_a = seed_agent(&pool, "经历测试甲").await;
    let agent_b = seed_agent(&pool, "经历测试乙").await;

    let base: i64 = TEST_TICK_BASE;
    // 甲：T+1 混含成功/失败两行（卡片级聚合的核心场景）
    seed_action(&pool, agent_a, base + 1, 0, "修炼", "success").await;
    seed_action(&pool, agent_a, base + 1, 1, "移动", "failed").await;
    // 甲：T+2 只有失败行（默认 success 筛选下不应入选）
    seed_action(&pool, agent_a, base + 2, 0, "移动", "failed").await;
    // 甲：T+3 成功行，位置不同（位置筛选的正样本）
    seed_action(&pool, agent_a, base + 3, 0, "交谈", "success").await;
    seed_state(&pool, agent_a, base + 3, "茶楼").await;
    // 乙：T+1 成功行（同一 tick 多角色）
    seed_action(&pool, agent_b, base + 1, 0, "修炼", "success").await;
    seed_state(&pool, agent_a, base, "龙门大堂").await;

    let agent_ids = [agent_a, agent_b];

    // 1) 绑定顺序错位会让下面任意一次调用直接报类型错误
    let all = ExperienceStreamFilters {
        result: "all",
        agent_id: Some(agent_a),
        location: None,
        action_type: None,
        from_tick: None,
        to_tick: None,
    };
    let count_all = count_experience_ticks(&pool, &all)
        .await
        .expect("count(all, agent_a) 必须能 Prepare");
    assert_eq!(count_all, 3, "甲的 3 个 tick 应各自算一张卡片");

    // 2) 卡片级聚合：混含成功/失败行的 tick 只算一张卡片
    let keys_all = fetch_experience_tick_keys(&pool, &all, 50, 0)
        .await
        .expect("keys(all, agent_a) 必须能 Prepare");
    assert_eq!(keys_all.len() as i64, count_all, "count 与 keys 必须同口径");
    let mut ticks: Vec<i64> = keys_all.iter().map(|(_, t)| *t).collect();
    let unique: std::collections::HashSet<(Uuid, i64)> = keys_all.iter().copied().collect();
    assert_eq!(unique.len(), keys_all.len(), "tick 键必须唯一");
    let mut sorted = ticks.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(ticks, sorted, "keys 必须按 tick_id DESC 稳定排序");
    ticks.clear();

    // 3) 默认（success）筛选按卡片判定：T+1 混含失败行，整张卡片不算成功
    let only_success = ExperienceStreamFilters {
        result: "success",
        ..all_with_none_location(Some(agent_a))
    };
    let count_success = count_experience_ticks(&pool, &only_success)
        .await
        .expect("count(success)");
    assert_eq!(
        count_success, 1,
        "只有 T+3 全部动作成功；T+1 混含失败行、T+2 全失败，都不算成功卡片"
    );

    // 4) failed 筛选同样按卡片判定：命中含任何失败行的 tick（T+1 混含、T+2 全失败）
    let only_failed = ExperienceStreamFilters {
        result: "failed",
        ..all_with_none_location(Some(agent_a))
    };
    let count_failed = count_experience_ticks(&pool, &only_failed)
        .await
        .expect("count(failed)");
    assert_eq!(count_failed, 2, "卡片级失败筛选命中 T+1 与 T+2 两张卡片");

    // 4b) 两个视图必须恰好划分全部卡片：筛选口径与徽章同口径的直接体现。
    // 行级筛选会让 T+1 同时出现在两个视图里（徽章却是失败），故此处是回归防线。
    assert_eq!(
        count_success + count_failed,
        count_all,
        "success 与 failed 视图必须无重叠、无遗漏地划分卡片"
    );

    // 4c) 未知 result 取值不匹配任何行（避免静默按行匹配的历史行为）
    let bogus = ExperienceStreamFilters {
        result: "bogus",
        ..all_with_none_location(Some(agent_a))
    };
    assert_eq!(
        count_experience_ticks(&pool, &bogus)
            .await
            .expect("count(bogus)"),
        0,
        "未知 result 取值应返回空集"
    );

    // 5) tick 区间筛选
    let ranged = ExperienceStreamFilters {
        from_tick: Some(base + 2),
        to_tick: Some(base + 3),
        ..all_with_none_location(Some(agent_a))
    };
    assert_eq!(
        count_experience_ticks(&pool, &ranged)
            .await
            .expect("count(range)"),
        2
    );

    // 6) 位置筛选：走 LATERAL 取该 tick 最近的 agent_states.node_id
    let at_teahouse = ExperienceStreamFilters {
        location: Some("茶楼"),
        ..all_with_none_location(Some(agent_a))
    };
    let keys_at_teahouse = fetch_experience_tick_keys(&pool, &at_teahouse, 50, 0)
        .await
        .expect("keys(location)");
    assert_eq!(keys_at_teahouse, vec![(agent_a, base + 3)]);

    // 7) 同一 tick 多角色：全库视角下 (agent_id, tick_id) 唯一。
    // 用 tick 区间把结果锁在 T+1，再按本测试自己的 agent_id 过滤，
    // 这样既不依赖分页顺序，也不会被库里他人的夹具干扰。
    let at_base_plus_1 = ExperienceStreamFilters {
        from_tick: Some(base + 1),
        to_tick: Some(base + 1),
        ..all_with_none_location(None)
    };
    let keys_at_t1 = fetch_experience_tick_keys(&pool, &at_base_plus_1, 100, 0)
        .await
        .expect("keys(global, T+1)");
    let mine_at_t1: Vec<(Uuid, i64)> = keys_at_t1
        .iter()
        .copied()
        .filter(|(a, _)| *a == agent_a || *a == agent_b)
        .collect();
    assert_eq!(mine_at_t1.len(), 2, "同一 tick 的两个角色各占一张卡片");
    assert!(
        mine_at_t1.iter().all(|(_, t)| *t == base + 1),
        "锁定的 tick 区间内只应返回 T+1 的卡片"
    );

    // 8) 动作类型筛选（六个绑定位置至此全部被覆盖）
    let only_talk = ExperienceStreamFilters {
        action_type: Some("交谈"),
        ..all_with_none_location(Some(agent_a))
    };
    assert_eq!(
        fetch_experience_tick_keys(&pool, &only_talk, 50, 0)
            .await
            .expect("keys(action_type)"),
        vec![(agent_a, base + 3)]
    );

    // 9) 越界页返回空集而不是报错
    let empty = fetch_experience_tick_keys(&pool, &at_base_plus_1, 10, 1_000_000)
        .await
        .expect("越界页不应报错");
    assert!(empty.is_empty());

    cleanup(&pool, &agent_ids).await;
}

/// 构造一个只改 agent_id 的默认筛选（result=all、无其它条件）
fn all_with_none_location(agent_id: Option<Uuid>) -> ExperienceStreamFilters<'static> {
    ExperienceStreamFilters {
        result: "all",
        agent_id,
        location: None,
        action_type: None,
        from_tick: None,
        to_tick: None,
    }
}
