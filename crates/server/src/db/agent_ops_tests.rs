//! agent_ops 模块单测（自 agent_ops.rs 外移，内容未改）

use super::{
    REBIRTH_FETCH_OLD_AGENT_SQL, REBIRTH_MARK_RETIRED_SQL, ROTATE_DEVICE_TOKEN_SQL,
    compute_rebirth_ticks,
};

/// 验证：state_tick 必须等于 caller 传入的世界 tick，不再用旧 agent 的
/// `MAX(agent_states.tick_id) + 1`。这是"重生即现在"的核心契约。
#[test]
fn test_compute_rebirth_ticks_uses_world_tick_for_state_tick() {
    assert_eq!(compute_rebirth_ticks(100, 10), (100, 90));
    assert_eq!(compute_rebirth_ticks(1, 0), (1, 1));
    assert_eq!(
        compute_rebirth_ticks(1_000_000, 5_000),
        (1_000_000, 995_000)
    );
}

/// 验证：starting_age_ticks == 0 时 birth_tick = world_tick，
/// 行为上等同于"新角色从世界 tick 出生，年龄从 0 起算"。
#[test]
fn test_compute_rebirth_ticks_zero_starting_age() {
    assert_eq!(compute_rebirth_ticks(42, 0), (42, 42));
}

/// 验证：oracle —— `birth_tick = world_tick - starting_age_ticks`，
/// 应保证 `compute_age_years(birth_tick, world_tick) == starting_age`。
/// 这是从 tick 推导年龄的可逆性测试。
#[test]
fn test_compute_rebirth_ticks_age_roundtrip() {
    let world_tick = 1234_i64;
    for starting_age in &[0_i64, 1, 10, 100, 1_000, 10_000] {
        let (state_tick, birth_tick) = compute_rebirth_ticks(world_tick, *starting_age);
        assert_eq!(state_tick, world_tick);
        assert_eq!(birth_tick, world_tick - starting_age);
        assert_eq!(world_tick - birth_tick, *starting_age);
    }
}

/// 验证：旧 agent 查询必须按 device_id 过滤，杜绝跨设备转世。
#[test]
fn test_p1_10_f2_rebirth_fetch_sql_filters_by_device_id() {
    let lower = REBIRTH_FETCH_OLD_AGENT_SQL.to_lowercase();
    assert!(
        lower.contains("where agent_id = $1"),
        "fetch SQL must bind old agent id at $1, got:\n{REBIRTH_FETCH_OLD_AGENT_SQL}"
    );
    assert!(
        lower.contains("and device_id = $2"),
        "fetch SQL 必须 AND device_id = $2 过滤，避免跨设备转世；got:\n{REBIRTH_FETCH_OLD_AGENT_SQL}"
    );
    assert!(
        lower.contains("and status = 'dead'"),
        "fetch SQL must filter by status='dead', got:\n{REBIRTH_FETCH_OLD_AGENT_SQL}"
    );
}

/// 验证：retired_at 标记必须用 IS NULL 守卫实现幂等。
#[test]
fn test_p1_10_f3_rebirth_mark_retired_sql_has_null_guard() {
    let lower = REBIRTH_MARK_RETIRED_SQL.to_lowercase();
    assert!(
        lower.contains("and retired_at is null"),
        "mark retired SQL 必须 AND retired_at IS NULL 守卫，阻断 agent retry 重复重生；got:\n{REBIRTH_MARK_RETIRED_SQL}"
    );
}

/// 验证：rotate_device_token 的 SQL 必须同时重置 token_created_at、
/// 写 token_rotated_at、RETURNING 新 token。这是后续接入
/// `retire_agent` / 调度器轮换 / 显式 endpoint 的基础。
#[test]
fn test_p1_12_rotate_device_token_sql_resets_timestamps_and_returns_new_token() {
    let lower = ROTATE_DEVICE_TOKEN_SQL.to_lowercase();
    assert!(
        lower.contains("update devices"),
        "rotate SQL must UPDATE devices table, got:\n{ROTATE_DEVICE_TOKEN_SQL}"
    );
    assert!(
        lower.contains("set auth_token = $2"),
        "必须 bind 新 token 到 $2，got:\n{ROTATE_DEVICE_TOKEN_SQL}"
    );
    assert!(
        lower.contains("token_created_at = now"),
        "必须重置 token_created_at = NOW()，got:\n{ROTATE_DEVICE_TOKEN_SQL}"
    );
    assert!(
        lower.contains("token_rotated_at = now"),
        "必须写 token_rotated_at = NOW()，got:\n{ROTATE_DEVICE_TOKEN_SQL}"
    );
    assert!(
        lower.contains("returning auth_token"),
        "必须 RETURNING auth_token 让调用方拿到新值，got:\n{ROTATE_DEVICE_TOKEN_SQL}"
    );
    assert!(
        lower.contains("where device_id = $1"),
        "必须按 device_id 过滤，got:\n{ROTATE_DEVICE_TOKEN_SQL}"
    );
}
