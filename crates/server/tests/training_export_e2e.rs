//! 端到端集成测试 (staging only, #[ignore] 默认跳过)
//!
//! 用真实 trace 数据 + 真实 PostgreSQL 跑一次完整 run_once, 验证:
//! - trace 扫描 → DB 查询 → attempt 精确匹配 → transform → 产物落盘 全链路
//! - 产物 JSONL 可被 serde 反序列化回 SftSample
//! - .meta.json 被正确写入
//! - checkpoint 记录了已处理的 trace_id
//!
//! 运行方式 (需 staging 环境):
//!   DATABASE_URL=postgres://postgres:...@localhost:5432/cyber_jianghu \
//!   CYBER_JIANGHU_DATA_DIR=crates/server/data \
//!   cargo test -p cyber-jianghu-server --test training_export_e2e -- --ignored --nocapture
//!
//! 不设 DATABASE_URL 时自动跳过 (返回 Ok), 不阻塞 CI.

#![cfg(test)]

use cyber_jianghu_server::training_export::checkpoint::Checkpoint;
use cyber_jianghu_server::training_export::config::TrainingExportConfig;
use cyber_jianghu_server::training_export::runner::run_once;
use cyber_jianghu_server::training_export::sft_transform::SftSample;
use cyber_jianghu_server::training_export::ExportRunRequest;
use sqlx::postgres::PgPoolOptions;

/// 读取 DATABASE_URL; 不存在则 skip.
fn db_url() -> Option<String> {
    let url = std::env::var("DATABASE_URL").ok()?;
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

#[tokio::test]
#[ignore]
async fn e2e_run_once_with_real_data_produces_valid_sft_jsonl() {
    let db_url = match db_url() {
        Some(u) => u,
        None => {
            eprintln!("跳过: DATABASE_URL 未设置");
            return;
        }
    };

    // 连真实 DB
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&db_url)
        .await
        .expect("连接 DB 失败");

    let config = TrainingExportConfig::default();
    let data_dir = cyber_jianghu_server::paths::get_data_dir();
    let traces_dir = data_dir.join(&config.paths.traces_input_subdir);

    // 确认有真实 trace 数据
    let trace_count = count_trace_entries(&traces_dir).await;
    if trace_count == 0 {
        eprintln!("跳过: {} 下无 trace 数据", traces_dir.display());
        return;
    }
    eprintln!("发现 {} 条 trace", trace_count);

    // 跑一次 run_once (scheduled 触发)
    let mut checkpoint = Checkpoint::default();
    let run_id = ulid::Ulid::new().to_string();
    let request = ExportRunRequest::scheduled(run_id.clone());
    let result = run_once(&config, &pool, &mut checkpoint, &request)
        .await
        .expect("run_once 失败");

    eprintln!(
        "run 完成: trace_count={}, sample_count={}, output={}",
        result.metadata.trace_count, result.metadata.sample_count, result.metadata.output_path
    );

    // 断言 1: metadata 状态正确
    assert_eq!(result.metadata.run_id, run_id);
    assert!(
        result.metadata.trace_count > 0,
        "应有 trace 被扫描到 (数据存在)"
    );

    // 断言 2: 产物文件真实存在且非空
    let output_path = data_dir.join(&result.metadata.output_path);
    assert!(output_path.exists(), "产物文件应存在: {:?}", output_path);
    let content = tokio::fs::read_to_string(&output_path)
        .await
        .expect("读产物失败");
    assert!(!content.is_empty(), "产物不应为空");

    // 断言 3: 每行是合法的 SftSample
    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), result.samples.len(), "产物行数应 == samples.len()");
    for (i, line) in lines.iter().enumerate() {
        let sample: SftSample = serde_json::from_str(line).unwrap_or_else(|e| {
            panic!("第 {} 行反序列化失败: {} | 内容: {}", i, e, &line[..line.len().min(200)])
        });
        // 每个样本至少有 user + assistant (system 可选)
        assert!(
            sample.messages.iter().any(|m| m.role == "user"),
            "sample {} 应有 user message",
            i
        );
        assert!(
            sample.messages.iter().any(|m| m.role == "assistant"),
            "sample {} 应有 assistant message",
            i
        );
        // assistant content 非空 (ok=false 的已被过滤)
        let asst = sample
            .messages
            .iter()
            .find(|m| m.role == "assistant")
            .unwrap();
        assert!(!asst.content.trim().is_empty(), "sample {} assistant 不应为空", i);
    }
    eprintln!("验证 {} 条 SftSample 全部合法", lines.len());

    // 断言 4: .meta.json 存在且可反序列化
    let meta_path = output_path.with_extension("jsonl").with_file_name(format!("run={}.meta.json", run_id));
    let meta_content = tokio::fs::read_to_string(&meta_path)
        .await
        .expect(".meta.json 应存在");
    let _: cyber_jianghu_server::training_export::RunMetadata =
        serde_json::from_str(&meta_content).expect(".meta.json 反序列化失败");
    eprintln!(".meta.json 验证通过");

    // 断言 5: checkpoint 记录了 trace_id (至少有一些)
    let total_processed: usize = checkpoint.buckets.values().map(|b| b.trace_ids.len()).sum();
    assert!(total_processed > 0, "checkpoint 应记录已处理 trace_id");
    eprintln!("checkpoint 记录 {} 个 trace_id", total_processed);

    // 清理产物 (测试隔离)
    let _ = tokio::fs::remove_file(&output_path).await;
    let _ = tokio::fs::remove_file(&meta_path).await;
    eprintln!("测试产物已清理");
}

/// 统计 traces_dir 下所有 .jsonl 的总行数
async fn count_trace_entries(traces_dir: &std::path::Path) -> usize {
    let mut total = 0;
    let Ok(mut agent_dirs) = tokio::fs::read_dir(traces_dir).await else {
        return 0;
    };
    while let Ok(Some(agent_entry)) = agent_dirs.next_entry().await {
        if !agent_entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(mut date_files) = tokio::fs::read_dir(agent_entry.path()).await else {
            continue;
        };
        while let Ok(Some(date_entry)) = date_files.next_entry().await {
            let path = date_entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false)
                && let Ok(content) = tokio::fs::read_to_string(&path).await
            {
                total += content.lines().filter(|l| !l.trim().is_empty()).count();
            }
        }
    }
    total
}
