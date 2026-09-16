//! runner 模块单测（自 runner.rs 外移，内容未改）

use super::scan_trace_files;
use uuid::Uuid;

#[tokio::test]
async fn scan_returns_empty_arrays_when_dir_absent() {
    // 若 traces_dir 不存在，scan 必须返回 (Vec, Vec, Vec)，run_once 的
    // if keys.is_empty() 分支会直接进入 empty-run 写 Completed meta 路径。
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("does-not-exist");
    let (entries, keys, dates) = scan_trace_files(&missing, 10, None, None)
        .await
        .expect("scan trace files on missing dir");
    assert!(entries.is_empty());
    assert!(keys.is_empty());
    assert!(dates.is_empty());
}

#[tokio::test]
async fn scan_drops_entries_mismatching_agent_filter() {
    // 准备 agent 目录与 trace 文件，agent_id_filter 不匹配时 entry 应被过滤。
    let temp = tempfile::tempdir().expect("tempdir");
    let traces_dir = temp.path().join("traces/soul=renhun");
    let agent_id_keep = Uuid::new_v4();
    let agent_id_drop = Uuid::new_v4();
    let keep_dir = traces_dir.join(format!("agent={agent_id_keep}"));
    let drop_dir = traces_dir.join(format!("agent={agent_id_drop}"));
    tokio::fs::create_dir_all(&keep_dir).await.unwrap();
    tokio::fs::create_dir_all(&drop_dir).await.unwrap();
    tokio::fs::write(
        keep_dir.join("date=2026-07-26.jsonl"),
        format!("{{\"trace_id\":\"a\",\"agent_id\":\"{agent_id_keep}\",\"character_name\":\"k\",\"tick_id\":1,\"soul_stage\":\"Renhun\",\"attempt\":0,\"provider\":\"p\",\"model\":\"m\",\"persona_name\":\"\",\"persona_description\":\"\",\"user_prompt\":\"u\",\"response\":\"r\",\"ok\":true,\"wall_clock\":1}}\n"),
    )
    .await
    .unwrap();
    tokio::fs::write(
        drop_dir.join("date=2026-07-26.jsonl"),
        format!("{{\"trace_id\":\"b\",\"agent_id\":\"{agent_id_drop}\",\"character_name\":\"d\",\"tick_id\":2,\"soul_stage\":\"Renhun\",\"attempt\":0,\"provider\":\"p\",\"model\":\"m\",\"persona_name\":\"\",\"persona_description\":\"\",\"user_prompt\":\"u\",\"response\":\"r\",\"ok\":true,\"wall_clock\":1}}\n"),
    )
    .await
    .unwrap();

    let (entries, _keys, _dates) = scan_trace_files(&traces_dir, 100, Some(agent_id_keep), None)
        .await
        .expect("scan with filter");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].agent_id, agent_id_keep);
    // 对照：不带 filter 时两条都被收集。
    let (entries_all, _, _) = scan_trace_files(&traces_dir, 100, None, None)
        .await
        .expect("scan without filter");
    assert_eq!(entries_all.len(), 2);
}

#[tokio::test]
async fn scan_skip_non_jsonl_and_invalid_date() {
    let temp = tempfile::tempdir().expect("tempdir");
    let traces_dir = temp.path().join("traces/soul=renhun");
    let agent_dir = traces_dir.join(format!("agent={}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&agent_dir).await.unwrap();
    tokio::fs::write(agent_dir.join("date=2026-07-26.jsonl"), "{}\n")
        .await
        .unwrap();
    tokio::fs::write(agent_dir.join("date=2026-07-26.json"), "{}\n")
        .await
        .unwrap();
    tokio::fs::write(agent_dir.join("date=2026-99-99.jsonl"), "{}\n")
        .await
        .unwrap();
    tokio::fs::write(agent_dir.join("README"), "noise")
        .await
        .unwrap();
    let (entries, _, _) = scan_trace_files(&traces_dir, 100, None, None)
        .await
        .expect("scan");
    assert!(entries.is_empty());
}

#[tokio::test]
async fn scan_respects_min_date_cutoff() {
    // 早于扫描下限的 trace（其 checkpoint 桶已退役）必须被跳过，
    // 否则每个调度周期都会重复导出；等于下限的当天保留。
    let temp = tempfile::tempdir().expect("tempdir");
    let traces_dir = temp.path().join("traces/soul=renhun");
    let agent_id = Uuid::new_v4();
    let agent_dir = traces_dir.join(format!("agent={agent_id}"));
    tokio::fs::create_dir_all(&agent_dir).await.unwrap();
    let line = format!(
        "{{\"trace_id\":\"t\",\"agent_id\":\"{agent_id}\",\"character_name\":\"c\",\"tick_id\":1,\"soul_stage\":\"Renhun\",\"attempt\":0,\"provider\":\"p\",\"model\":\"m\",\"persona_name\":\"\",\"persona_description\":\"\",\"user_prompt\":\"u\",\"response\":\"r\",\"ok\":true,\"wall_clock\":1}}\n"
    );
    tokio::fs::write(agent_dir.join("date=2026-07-01.jsonl"), &line)
        .await
        .unwrap();
    tokio::fs::write(agent_dir.join("date=2026-07-26.jsonl"), &line)
        .await
        .unwrap();

    let (entries, _, _) = scan_trace_files(&traces_dir, 100, None, Some("2026-07-26".to_string()))
        .await
        .expect("scan with cutoff");
    assert_eq!(entries.len(), 1, "只有 >= 下限的日期文件被扫描");
}
