//! soul_cycle_recorder 模块单测（自 soul_cycle_recorder.rs 外移，内容未改）

use super::*;
use tempfile::TempDir;

fn make_recorder() -> (TempDir, SoulCycleRecorder) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("soul_cycle.db");
    let recorder = SoulCycleRecorder::open(Uuid::new_v4(), &db_path).unwrap();
    (temp_dir, recorder)
}

#[tokio::test]
async fn test_record_renhun() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_renhun(1, 0, "吃馒头充饥", "思考中...", "test-model")
        .await;
    let records = recorder.get_by_tick(1).await.expect("get_by_tick in test");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].renhun_narrative.as_deref(), Some("吃馒头充饥"));
    assert_eq!(records[0].renhun_thought_log.as_deref(), Some("思考中..."));
}

#[tokio::test]
async fn test_tianhun_layers_column() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_renhun(1, 0, "吃馒头", "...", "test-model")
        .await;
    recorder
        .record_tianhun(
            1,
            0,
            "approved",
            Some("目标校验通过"),
            Some("action_type合法"),
            Some("物品存在"),
            None,
            None,
            None,
        )
        .await;
    let records = recorder.get_by_tick(1).await.expect("get_by_tick");
    let record = &records[0];
    // 新列 tianhun_layers 应有值
    assert!(
        record.tianhun_layers.is_some(),
        "tianhun_layers should be set"
    );
    let layers: Vec<serde_json::Value> =
        serde_json::from_str(record.tianhun_layers.as_ref().unwrap()).expect("tianhun_layers JSON");
    assert_eq!(layers.len(), 3, "should have 3 layers (layer0-2)");
    assert_eq!(layers[0]["layer"], "layer0");
    assert_eq!(layers[1]["layer"], "layer1");
    assert!(layers[1]["passed"].as_bool().unwrap());
    // 旧列仍然兼容
    assert_eq!(
        record.tianhun_layer1_result.as_deref(),
        Some("action_type合法")
    );
}

#[tokio::test]
async fn test_server_execution_results_column() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_renhun(1, 0, "吃馒头", "...", "test-model")
        .await;
    let exec_results = r#"{"0":{"success":true,"error":null,"state_change_summary":"体力+5"}}"#;
    recorder.backfill_server_result(1, 0, exec_results).await;
    let records = recorder.get_by_tick(1).await.expect("get_by_tick");
    assert_eq!(
        records[0].server_execution_results.as_deref(),
        Some(exec_results)
    );
}

#[tokio::test]
async fn test_record_tianhun_approved() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_renhun(1, 0, "吃馒头", "...", "test-model")
        .await;
    recorder
        .record_tianhun(
            1,
            0,
            "approved",
            None,
            Some("action_type合法"),
            Some("物品存在"),
            None,
            None,
            None,
        )
        .await;
    let records = recorder.get_by_tick(1).await.expect("get_by_tick in test");
    assert_eq!(records[0].tianhun_result.as_deref(), Some("approved"));
    assert_eq!(
        records[0].tianhun_layer1_result.as_deref(),
        Some("action_type合法")
    );
}

#[tokio::test]
async fn test_record_tianhun_rejected() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_renhun(1, 0, "无效", "...", "test-model")
        .await;
    recorder
        .record_tianhun(
            1,
            0,
            "rejected",
            Some("目标不可见"),
            Some("action_type合法"),
            None,
            None,
            Some("意图不合理"),
            None,
        )
        .await;
    let records = recorder.get_by_tick(1).await.expect("get_by_tick in test");
    assert_eq!(records[0].tianhun_result.as_deref(), Some("rejected"));
    assert_eq!(records[0].tianhun_reason.as_deref(), Some("意图不合理"));
}

#[tokio::test]
async fn test_unique_constraint_tick_attempt() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_renhun(1, 0, "第一次", "...", "test-model")
        .await;
    recorder
        .record_renhun(1, 0, "第二次覆盖", "...", "test-model")
        .await;
    let records = recorder.get_by_tick(1).await.expect("get_by_tick in test");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].renhun_narrative.as_deref(), Some("第二次覆盖"));
}

#[tokio::test]
async fn test_record_immediate() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_immediate(
            1,
            "uuid123",
            Some("和人打招呼"),
            "extracted",
            "说话",
            Some(r#"{"content":"你好"}"#),
            Some("你好"),
            "sent",
            None,
        )
        .await;
    let records = recorder
        .get_immediate_by_tick(1)
        .await
        .expect("get_immediate_by_tick in test");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].action_type, "说话");
    assert_eq!(records[0].send_status, "sent");
}

#[tokio::test]
async fn test_record_immediate_failed() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_immediate(
            1,
            "uuid456",
            Some("喊话"),
            "pure",
            "说话",
            Some(r#"{"content":"救命"}"#),
            Some("救命"),
            "failed",
            Some("WebSocket 断开"),
        )
        .await;
    let records = recorder
        .get_immediate_by_tick(1)
        .await
        .expect("get_immediate_by_tick in test");
    assert_eq!(records[0].send_status, "failed");
    assert_eq!(records[0].send_error.as_deref(), Some("WebSocket 断开"));
}

#[tokio::test]
async fn test_world_time() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_renhun(1, 0, "移动", "...", "test-model")
        .await;
    recorder.record_world_time(1, 0, "第三天 申时").await;
    let records = recorder.get_by_tick(1).await.expect("get_by_tick in test");
    assert_eq!(records[0].world_time.as_deref(), Some("第三天 申时"));
}

#[tokio::test]
async fn test_get_tick_ids_page_dedup_and_order() {
    let (_dir, recorder) = make_recorder();
    // tick 1 有 2 次 attempt，tick 2 和 3 各 1 次
    recorder
        .record_renhun(1, 0, "a1", "...", "test-model")
        .await;
    recorder
        .record_renhun(1, 1, "a2", "...", "test-model")
        .await;
    recorder.record_renhun(3, 0, "c", "...", "test-model").await;
    recorder.record_renhun(2, 0, "b", "...", "test-model").await;

    let (ids, total) = recorder
        .get_tick_ids_page(1, 10)
        .await
        .expect("get_tick_ids_page in test");
    assert_eq!(total, 3);
    assert_eq!(ids, vec![3, 2, 1]); // 降序，tick 1 只出现一次
}

#[tokio::test]
async fn test_get_tick_ids_page_pagination() {
    let (_dir, recorder) = make_recorder();
    for i in 1..=5 {
        recorder
            .record_renhun(i, 0, &format!("n{}", i), "...", "test-model")
            .await;
    }

    let (p1, total) = recorder
        .get_tick_ids_page(1, 3)
        .await
        .expect("get_tick_ids_page p1 in test");
    let (p2, _) = recorder
        .get_tick_ids_page(2, 3)
        .await
        .expect("get_tick_ids_page p2 in test");
    assert_eq!(total, 5);
    assert_eq!(p1, vec![5, 4, 3]);
    assert_eq!(p2, vec![2, 1]);
}

#[tokio::test]
async fn test_get_tick_ids_page_empty() {
    let (_dir, recorder) = make_recorder();
    let (ids, total) = recorder
        .get_tick_ids_page(1, 10)
        .await
        .expect("get_tick_ids_page empty in test");
    assert!(ids.is_empty());
    assert_eq!(total, 0);
}

#[tokio::test]
async fn test_get_by_ticks_batch() {
    let (_dir, recorder) = make_recorder();
    recorder.record_renhun(1, 0, "a", "...", "test-model").await;
    recorder
        .record_renhun(1, 1, "a2", "...", "test-model")
        .await;
    recorder.record_renhun(3, 0, "c", "...", "test-model").await;
    // tick 2 不存在

    let records = recorder
        .get_by_ticks(&[1, 2, 3])
        .await
        .expect("get_by_ticks in test");
    assert_eq!(records.len(), 3); // tick1×2 + tick3×1
    assert_eq!(records[0].tick_id, 3); // 降序
    assert_eq!(records[1].tick_id, 1);
    assert_eq!(records[2].tick_id, 1);
}

#[tokio::test]
async fn test_get_by_ticks_empty() {
    let (_dir, recorder) = make_recorder();
    let records = recorder
        .get_by_ticks(&[])
        .await
        .expect("get_by_ticks empty in test");
    assert!(records.is_empty());
}

#[tokio::test]
async fn test_get_by_ticks_batches_over_batch_size() {
    let (_dir, recorder) = make_recorder();
    // 250 个 tick（跨 3 批）：超批量不得静默返空（传记经历日志失效根因回归）
    for t in 1..=250i64 {
        recorder
            .record_renhun(t, 0, &format!("n{t}"), "...", "test-model")
            .await;
    }

    let tick_ids: Vec<i64> = (1..=250).collect();
    let records = recorder
        .get_by_ticks(&tick_ids)
        .await
        .expect("get_by_ticks over batch size in test");
    assert_eq!(records.len(), 250);
    // 合并后保持 tick_id 降序
    assert_eq!(records[0].tick_id, 250);
    assert_eq!(records[249].tick_id, 1);
}

#[tokio::test]
async fn test_get_immediate_by_ticks_batch() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_immediate(
            1,
            "id1",
            None,
            "extracted",
            "说话",
            None,
            Some("hi"),
            "sent",
            None,
        )
        .await;
    recorder
        .record_immediate(
            3,
            "id2",
            None,
            "pure",
            "说话",
            None,
            Some("bye"),
            "sent",
            None,
        )
        .await;
    recorder
        .record_immediate(
            3,
            "id3",
            None,
            "pure",
            "说话",
            None,
            Some("yo"),
            "failed",
            Some("err"),
        )
        .await;

    let records = recorder
        .get_immediate_by_ticks(&[1, 2, 3])
        .await
        .expect("get_immediate_by_ticks in test");
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].tick_id, 1);
    assert_eq!(records[1].tick_id, 3);
    assert_eq!(records[2].tick_id, 3);
}

#[tokio::test]
async fn test_get_immediate_by_ticks_batches_over_batch_size() {
    let (_dir, recorder) = make_recorder();
    // 250 个 tick（跨 3 批）：超批量不得静默返空，合并后按 id 升序
    for t in 1..=250i64 {
        recorder
            .record_immediate(
                t,
                &format!("id{t}"),
                None,
                "pure",
                "说话",
                None,
                Some("hi"),
                "sent",
                None,
            )
            .await;
    }

    let tick_ids: Vec<i64> = (1..=250).collect();
    let records = recorder
        .get_immediate_by_ticks(&tick_ids)
        .await
        .expect("get_immediate_by_ticks over batch size in test");
    assert_eq!(records.len(), 250);
    assert!(records.windows(2).all(|w| w[0].id <= w[1].id));
}

// ========================================================================
// 闭环：7 个 query 方法必须返回 Result，DB 错时 caller 显式处理
// 模式同（outcome.rs）：init → DROP TABLE → assert Err
// 之前静默返回 None / 空 Vec / (空, 0) 会让 caller 误以为"无数据"
// ========================================================================

fn break_db(db: &std::path::Path) {
    let conn = rusqlite::Connection::open(db).expect("reopen db");
    conn.execute("DROP TABLE soul_cycle_record", [])
        .expect("drop soul_cycle_record");
    conn.execute("DROP TABLE immediate_intent_record", [])
        .expect("drop immediate_intent_record");
}

#[tokio::test]
async fn test_p0_audit_get_last_recorded_tick_returns_err_on_broken_db() {
    let (dir, recorder) = make_recorder();
    break_db(&dir.path().join("soul_cycle.db"));
    let result = recorder.get_last_recorded_tick(100).await;
    assert!(
        result.is_err(),
        "get_last_recorded_tick 在 DB 损坏时必须返回 Err，caller 显式处理"
    );
}

#[tokio::test]
async fn test_p0_audit_get_last_renhun_narrative_returns_err_on_broken_db() {
    let (dir, recorder) = make_recorder();
    break_db(&dir.path().join("soul_cycle.db"));
    let result = recorder.get_last_renhun_narrative(100).await;
    assert!(
        result.is_err(),
        "get_last_renhun_narrative 在 DB 损坏时必须返回 Err"
    );
}

#[tokio::test]
async fn test_p0_audit_get_by_tick_returns_err_on_broken_db() {
    let (dir, recorder) = make_recorder();
    break_db(&dir.path().join("soul_cycle.db"));
    let result = recorder.get_by_tick(1).await;
    assert!(
        result.is_err(),
        "get_by_tick 在 DB 损坏时必须返回 Err，caller 显式处理"
    );
}

#[tokio::test]
async fn test_p0_audit_get_tick_ids_page_returns_err_on_broken_db() {
    let (dir, recorder) = make_recorder();
    break_db(&dir.path().join("soul_cycle.db"));
    let result = recorder.get_tick_ids_page(1, 10).await;
    assert!(
        result.is_err(),
        "get_tick_ids_page 在 DB 损坏时必须返回 Err"
    );
}

#[tokio::test]
async fn test_p0_audit_get_by_ticks_returns_err_on_broken_db() {
    let (dir, recorder) = make_recorder();
    break_db(&dir.path().join("soul_cycle.db"));
    let result = recorder.get_by_ticks(&[1, 2, 3]).await;
    assert!(result.is_err(), "get_by_ticks 在 DB 损坏时必须返回 Err");
}

#[tokio::test]
async fn test_p0_audit_get_immediate_by_ticks_returns_err_on_broken_db() {
    let (dir, recorder) = make_recorder();
    break_db(&dir.path().join("soul_cycle.db"));
    let result = recorder.get_immediate_by_ticks(&[1, 2, 3]).await;
    assert!(
        result.is_err(),
        "get_immediate_by_ticks 在 DB 损坏时必须返回 Err"
    );
}

#[tokio::test]
async fn test_p0_audit_get_immediate_by_tick_returns_err_on_broken_db() {
    let (dir, recorder) = make_recorder();
    break_db(&dir.path().join("soul_cycle.db"));
    let result = recorder.get_immediate_by_tick(1).await;
    assert!(
        result.is_err(),
        "get_immediate_by_tick 在 DB 损坏时必须返回 Err"
    );
}

#[tokio::test]
async fn test_record_idle_skip_writes_distinguishable_placeholder() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_idle_skip(
            7,
            "（空转：无显著变化，未执行认知循环）",
            Some("第三天 申时"),
            "test-model",
        )
        .await;

    let records = recorder.get_by_tick(7).await.expect("get_by_tick in test");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].route_type, "idle_skip");
    assert_eq!(records[0].world_time.as_deref(), Some("第三天 申时"));
    assert_eq!(
        records[0].renhun_narrative.as_deref(),
        Some("（空转：无显著变化，未执行认知循环）")
    );
    assert_eq!(records[0].tianhun_result, None, "占位行不得伪装天魂结果");
    assert_eq!(
        records[0].model_id.as_deref(),
        Some("test-model"),
        "空转 tick 必须上报角色当前活跃模型，否则经历日志出现有经历无模型的空列"
    );
}

#[tokio::test]
async fn test_model_id_normalization_treats_empty_and_unknown_as_unreported() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_renhun(1, 0, "甲", "...", "MiniMax-M2.7")
        .await;
    recorder.record_renhun(2, 0, "乙", "...", "").await;
    recorder.record_renhun(3, 0, "丙", "...", "unknown").await;
    recorder
        .record_idle_skip(4, "（空转）", None, "unknown")
        .await;

    assert_eq!(
        recorder.get_by_tick(1).await.expect("tick 1")[0]
            .model_id
            .as_deref(),
        Some("MiniMax-M2.7")
    );
    for tick in [2, 3, 4] {
        assert_eq!(
            recorder.get_by_tick(tick).await.expect("tick")[0].model_id,
            None,
            "空串与占位符 unknown 必须归一为 NULL（tick {tick}）"
        );
    }
}

#[tokio::test]
async fn test_record_idle_skip_never_overwrites_cognitive_record() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_renhun(7, 0, "真实决策", "...", "test-model")
        .await;
    recorder
        .record_idle_skip(7, "（空转）", None, "test-model")
        .await;

    let records = recorder.get_by_tick(7).await.expect("get_by_tick in test");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].route_type, "main");
    assert_eq!(records[0].renhun_narrative.as_deref(), Some("真实决策"));
}

#[tokio::test]
async fn test_record_renhun_flips_idle_row_back_to_main() {
    let (_dir, recorder) = make_recorder();
    // 回归防线（triple-review 建议2/F4）：同一 tick 先空转后认知时，
    // 认知 upsert 必须把 route_type 翻回 main，真实叙事不得滞留 idle_skip 行
    recorder
        .record_idle_skip(7, "（空转）", None, "test-model")
        .await;
    recorder
        .record_renhun(7, 0, "真实决策", "...", "test-model")
        .await;

    let records = recorder.get_by_tick(7).await.expect("get_by_tick in test");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].route_type, "main");
    assert_eq!(records[0].renhun_narrative.as_deref(), Some("真实决策"));
}

#[tokio::test]
async fn test_get_last_renhun_narrative_ignores_idle_placeholder() {
    let (_dir, recorder) = make_recorder();
    recorder
        .record_renhun(10, 0, "真实行动", "...", "test-model")
        .await;
    recorder
        .record_idle_skip(12, "（空转）", None, "test-model")
        .await;

    let narrative = recorder
        .get_last_renhun_narrative(20)
        .await
        .expect("get_last_renhun_narrative in test");
    assert_eq!(
        narrative.as_deref(),
        Some("真实行动"),
        "「上一轮的行动」通道不得把空转占位当作真实行动"
    );
}
