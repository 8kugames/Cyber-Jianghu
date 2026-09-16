//! outcome 模块单测（自 outcome.rs 外移，内容未改）

use super::*;
use std::path::PathBuf;

fn temp_db() -> PathBuf {
    std::env::temp_dir().join(format!("outcome_test_{}.db", uuid::Uuid::new_v4()))
}

#[test]
fn test_record_and_query() {
    let db = temp_db();
    let mem = OutcomeMemory::new(&db, 10).unwrap();

    mem.record(OutcomeRecord {
        action_type: "用".into(),
        action_data: Some(serde_json::json!({"item_id": "馒头"})),
        result: OutcomeResult::Success,
        target_agent_id: None,
        context_hash: "龙门大堂:food,drink:2".into(),
        tick_id: 100,
    })
    .expect("record must succeed in test");

    mem.record(OutcomeRecord {
        action_type: "用".into(),
        action_data: Some(serde_json::json!({"item_id": "invalid"})),
        result: OutcomeResult::Failed("物品不存在".into()),
        target_agent_id: None,
        context_hash: "龙门大堂:food,drink:2".into(),
        tick_id: 101,
    })
    .expect("record must succeed in test");

    let records = mem.query_recent("用", 10).expect("query_recent in test");
    assert_eq!(records.len(), 2);
    assert!(matches!(records[0].result, OutcomeResult::Failed(_)));
    assert!(matches!(records[1].result, OutcomeResult::Success));

    let rate = mem.success_rate("用").expect("success_rate in test");
    assert!((rate - 0.5).abs() < 0.01);

    let _ = std::fs::remove_file(&db);
}

#[test]
fn test_prompt_context() {
    let db = temp_db();
    let mem = OutcomeMemory::new(&db, 10).unwrap();

    mem.record(OutcomeRecord {
        action_type: "移动".into(),
        action_data: Some(serde_json::json!({"target_location": "龙门厨房"})),
        result: OutcomeResult::Success,
        target_agent_id: None,
        context_hash: "龙门大堂::1".into(),
        tick_id: 100,
    })
    .expect("record must succeed in test");

    let ctx = mem.to_prompt_context();
    assert!(ctx.contains("经验教训"));
    assert!(ctx.contains("移动 → 成功"));

    let _ = std::fs::remove_file(&db);
}

#[test]
fn test_dynamic_action_types() {
    let db = temp_db();
    let mem = OutcomeMemory::new(&db, 10).unwrap();

    mem.record(OutcomeRecord {
        action_type: "攻击".into(),
        action_data: None,
        result: OutcomeResult::Success,
        target_agent_id: None,
        context_hash: "loc::0".into(),
        tick_id: 100,
    })
    .expect("record must succeed in test");

    let types = mem
        .distinct_action_types()
        .expect("distinct_action_types in test");
    assert!(types.contains(&"攻击".to_string()));

    let ctx = mem.to_prompt_context();
    assert!(ctx.contains("攻击 → 成功"));

    let _ = std::fs::remove_file(&db);
}

#[test]
fn test_query_by_target() {
    let db = temp_db();
    let mem = OutcomeMemory::new(&db, 10).unwrap();

    let target_id = "agent-b";
    mem.record(OutcomeRecord {
        action_type: "予".into(),
        action_data: Some(
            serde_json::json!({"item_id": "馒头", "quantity": 10, "target_agent_id": target_id}),
        ),
        result: OutcomeResult::Success,
        target_agent_id: Some(target_id.to_string()),
        context_hash: "loc::1".into(),
        tick_id: 100,
    })
    .expect("record must succeed in test");
    mem.record(OutcomeRecord {
        action_type: "予".into(),
        action_data: None,
        result: OutcomeResult::Success,
        target_agent_id: Some(target_id.to_string()),
        context_hash: "loc::1".into(),
        tick_id: 101,
    })
    .expect("record must succeed in test");
    mem.record(OutcomeRecord {
        action_type: "攻击".into(),
        action_data: None,
        result: OutcomeResult::Success,
        target_agent_id: Some("agent-c".to_string()),
        context_hash: "loc::1".into(),
        tick_id: 102,
    })
    .expect("record must succeed in test");

    let records = mem
        .query_by_target(target_id, 10)
        .expect("query_by_target in test");
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|r| r.target_agent_id.as_deref() == Some(target_id))
    );

    let records_c = mem
        .query_by_target("agent-c", 10)
        .expect("query_by_target in test");
    assert_eq!(records_c.len(), 1);

    let records_none = mem
        .query_by_target("nonexistent", 10)
        .expect("query_by_target in test");
    assert!(records_none.is_empty());

    let _ = std::fs::remove_file(&db);
}

#[test]
fn test_extract_target_agent_id() {
    assert_eq!(
        extract_target_agent_id(&Some(serde_json::json!({"target_agent_id": "abc"}))),
        Some("abc".to_string())
    );
    assert_eq!(
        extract_target_agent_id(&Some(serde_json::json!({"target_id": "def"}))),
        Some("def".to_string())
    );
    assert_eq!(
        extract_target_agent_id(&Some(serde_json::json!({"target_uuid": "ghi"}))),
        Some("ghi".to_string())
    );
    assert_eq!(
        extract_target_agent_id(&Some(serde_json::json!({"item_id": "馒头"}))),
        None
    );
    assert_eq!(extract_target_agent_id(&None), None);
}

#[test]
fn test_prompt_context_per_target() {
    let db = temp_db();
    let mem = OutcomeMemory::new(&db, 10).unwrap();

    // 有 target 的动作
    mem.record(OutcomeRecord {
        action_type: "予".into(),
        action_data: None,
        result: OutcomeResult::Success,
        target_agent_id: Some("npc-a".to_string()),
        context_hash: "loc::1".into(),
        tick_id: 100,
    })
    .expect("record must succeed in test");
    mem.record(OutcomeRecord {
        action_type: "予".into(),
        action_data: None,
        result: OutcomeResult::Failed("物品不足".into()),
        target_agent_id: Some("npc-a".to_string()),
        context_hash: "loc::1".into(),
        tick_id: 101,
    })
    .expect("record must succeed in test");
    mem.record(OutcomeRecord {
        action_type: "予".into(),
        action_data: None,
        result: OutcomeResult::Success,
        target_agent_id: Some("npc-b".to_string()),
        context_hash: "loc::1".into(),
        tick_id: 102,
    })
    .expect("record must succeed in test");
    // 无 target 的动作
    mem.record(OutcomeRecord {
        action_type: "用".into(),
        action_data: None,
        result: OutcomeResult::Success,
        target_agent_id: None,
        context_hash: "loc::1".into(),
        tick_id: 103,
    })
    .expect("record must succeed in test");

    let ctx = mem.to_prompt_context();
    assert!(
        ctx.contains("予 npc-a"),
        "should contain per-target line: {}",
        ctx
    );
    assert!(
        ctx.contains("予 npc-b"),
        "should contain per-target line: {}",
        ctx
    );
    assert!(
        ctx.contains("用"),
        "should contain no-target action: {}",
        ctx
    );
    assert!(
        !ctx.contains("予 →"),
        "should NOT contain action-only line: {}",
        ctx
    );

    let _ = std::fs::remove_file(&db);
}

#[test]
fn test_outcome_memory_migrates_legacy_db_without_target_column() {
    // 模拟老库：只建表，缺 target_agent_id 列
    let db = temp_db();
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TABLE outcome_records (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            action_type      TEXT NOT NULL,
            action_data      TEXT,
            result_type      TEXT NOT NULL,
            result           TEXT,
            context_hash     TEXT NOT NULL,
            tick_id          INTEGER NOT NULL,
            created_at       INTEGER DEFAULT (strftime('%s', 'now'))
        )",
    )
    .unwrap();
    drop(conn);

    // 旧版 OutcomeMemory 初始化会"成功"地假装完成迁移；新版必须真正补列
    let _mem = OutcomeMemory::new(&db, 10).expect("legacy migration should succeed");
    drop(_mem);
    let conn = Connection::open(&db).unwrap();
    let mut stmt = conn.prepare("PRAGMA table_info(outcome_records)").unwrap();
    let mut rows = stmt.query([]).unwrap();
    let mut has_target = false;
    while let Some(row) = rows.next().unwrap() {
        let name: String = row.get(1).unwrap();
        if name == "target_agent_id" {
            has_target = true;
        }
    }
    assert!(
        has_target,
        "legacy DB should be migrated to include target_agent_id"
    );
    let _ = std::fs::remove_file(&db);
}

#[test]
fn test_outcome_memory_init_is_idempotent_for_already_migrated_db() {
    // 全新建库、再 init、再 init：第二次不应失败
    let db = temp_db();
    let _first = OutcomeMemory::new(&db, 10).expect("first init");
    let _second = OutcomeMemory::new(&db, 10).expect("second init must be idempotent");
    let _ = std::fs::remove_file(&db);
}

// ========================================================================
// silent error visibility 测试
// 测试方法：先 init OutcomeMemory（建表），再从外部 DROP TABLE，
// 让 cached conn 的下个 query 失败（"no such table"）。
// 修复前：返回空 Vec / 0.0 / ()，不报错 → 静默吞错
// 修复后：返回 Err，由 caller 决定如何处理
// ========================================================================

fn break_db_by_dropping_table(db: &std::path::Path) {
    let conn = rusqlite::Connection::open(db).expect("reopen db to break it");
    conn.execute("DROP TABLE outcome_records", [])
        .expect("drop outcome_records from underneath");
}

/// 验证：record() 必须返回 Result，DB 错时返回 Err。
/// 之前用 `if let Err(e) = ... { debug!(...) }` 静默吞错，且 debug 级
/// 默认不输出，运维完全看不到。
#[test]
fn test_p1_3_record_returns_err_on_broken_db() {
    let db = temp_db();
    let mem = OutcomeMemory::new(&db, 10).expect("new");
    break_db_by_dropping_table(&db);

    let result = mem.record(OutcomeRecord {
        action_type: "test".into(),
        action_data: None,
        result: OutcomeResult::Success,
        target_agent_id: None,
        context_hash: "ctx".into(),
        tick_id: 1,
    });
    assert!(
        result.is_err(),
        "record() 在 DB 损坏时必须返回 Err，而非静默吞错。\
         当前 is_ok={}",
        result.is_ok()
    );
    let _ = std::fs::remove_file(&db);
}

/// 验证：query_recent() 必须返回 Result，DB 错时返回 Err。
/// 之前返回空 Vec 会让 caller 误以为"无记录"而非"DB 坏"——这是误导。
#[test]
fn test_p1_3_query_recent_returns_err_on_broken_db() {
    let db = temp_db();
    let mem = OutcomeMemory::new(&db, 10).expect("new");
    break_db_by_dropping_table(&db);

    let result = mem.query_recent("test", 10);
    assert!(
        result.is_err(),
        "query_recent() 在 DB 损坏时必须返回 Err，\
         而非静默返回空 Vec（会让 caller 把 DB 错当成'无记录'）"
    );
    let _ = std::fs::remove_file(&db);
}

/// 验证：query_by_target() 必须返回 Result。
#[test]
fn test_p1_3_query_by_target_returns_err_on_broken_db() {
    let db = temp_db();
    let mem = OutcomeMemory::new(&db, 10).expect("new");
    break_db_by_dropping_table(&db);

    let result = mem.query_by_target("target", 10);
    assert!(
        result.is_err(),
        "query_by_target() 在 DB 损坏时必须返回 Err"
    );
    let _ = std::fs::remove_file(&db);
}

/// 验证：success_rate() 必须返回 Result，DB 错时返回 Err。
/// 之前返回 0.0 会让 caller 误以为"零成功率"而非"DB 错"——这是误导。
#[test]
fn test_p1_3_success_rate_returns_err_on_broken_db() {
    let db = temp_db();
    let mem = OutcomeMemory::new(&db, 10).expect("new");
    break_db_by_dropping_table(&db);

    let result = mem.success_rate("test");
    assert!(
        result.is_err(),
        "success_rate() 在 DB 损坏时必须返回 Err，\
         而非静默返回 0.0（会把 DB 错当成'全失败'）"
    );
    let _ = std::fs::remove_file(&db);
}

/// 验证：to_prompt_context() 必须在 query 失败时降级为空字符串（或部分内容），
/// **不 panic** 且不 block 调用方。caller 期望的契约是 best-effort：
/// DB 错 → 空内容 + 警告日志，而非 panic / 阻断主流程。
#[test]
fn test_p1_3_to_prompt_context_returns_empty_on_broken_db_without_panic() {
    let db = temp_db();
    let mem = OutcomeMemory::new(&db, 10).expect("new");
    break_db_by_dropping_table(&db);

    // to_prompt_context 的签名不变（仍返回 String），但内部必须显式处理
    // query 失败：warn! 记录后返回空或部分内容，**不 panic**。
    let ctx = mem.to_prompt_context();
    // 行为契约：返回空字符串（无 records 字段）
    assert!(
        ctx.is_empty() || !ctx.contains("record:"),
        "to_prompt_context 在 DB 损坏时必须降级为空/部分内容，\
         不 panic 且无假数据。当前返回：{ctx}"
    );
    let _ = std::fs::remove_file(&db);
}
