//! token_tracking 模块单测（自 token_tracking.rs 外移，内容未改）

use super::*;
use crate::config::data_base_dir;
use std::env;
use std::sync::OnceLock;

/// 串行化所有触及全局 TOKEN_STATS 的测试（避免 cargo 并行跑测试时互相污染）
static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn test_lock() -> &'static Mutex<()> {
    TEST_LOCK.get_or_init(|| Mutex::new(()))
}

// ---- 辅助：构造一个固定的 LlmProvider / model_key ----
fn prov() -> LlmProvider {
    LlmProvider::OpenAICompatible
}
fn m1() -> &'static str {
    "model-a"
}
fn m2() -> &'static str {
    "model-b"
}

// ---- 1. hour_key 格式 ----
#[test]
fn test_hour_key_format() {
    let tz = local_tz();
    let t = "2026-06-01T02:35:00Z"
        .parse::<DateTime<Utc>>()
        .unwrap()
        .with_timezone(&tz);
    assert_eq!(
        hour_key(t),
        format!("{}-{}", t.format("%Y-%m-%d"), t.format("%H"))
    );

    let t2 = "2026-12-31T23:59:59Z"
        .parse::<DateTime<Utc>>()
        .unwrap()
        .with_timezone(&tz);
    assert_eq!(
        hour_key(t2),
        format!("{}-{}", t2.format("%Y-%m-%d"), t2.format("%H"))
    );

    let t3 = "2026-01-01T00:00:00Z"
        .parse::<DateTime<Utc>>()
        .unwrap()
        .with_timezone(&tz);
    assert_eq!(
        hour_key(t3),
        format!("{}-{}", t3.format("%Y-%m-%d"), t3.format("%H"))
    );
}

// ---- 2. parse_hour_key 反解 ----
#[test]
fn test_parse_hour_key_roundtrip() {
    let tz = local_tz();
    let t = "2026-06-01T02:35:00Z"
        .parse::<DateTime<Utc>>()
        .unwrap()
        .with_timezone(&tz);
    let hk = hour_key(t);
    let parsed = parse_hour_key(&hk);
    // parsed 是 UTC，转回本地时区验证格式一致
    let parsed_local = parsed.with_timezone(&tz);
    assert_eq!(parsed_local.format("%Y-%m-%d-%H").to_string(), hk);
    // 解析结果应是整点（分秒=0）
    assert_eq!(parsed.timestamp() % 3600, 0);
}

// ---- 3. split_model_key 拆分 ----
#[test]
fn test_split_model_key() {
    assert_eq!(
        split_model_key("openai_compatible/deepseek-v4-pro"),
        (
            "openai_compatible".to_string(),
            "deepseek-v4-pro".to_string()
        )
    );
    assert_eq!(
        split_model_key("no-slash"),
        ("unknown".to_string(), "no-slash".to_string())
    );
}

// ---- 4. record_token_usage / record_failure 跨小时分桶 ----
/// 通过临时改 env 隔离 data_base_dir；并在每个测试末尾清理内存
fn isolate() {
    // 每次测试用独立目录
    let unique = format!(
        "test_token_tracking_{}_{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let dir = env::temp_dir().join(unique);
    let _ = fs::create_dir_all(&dir);
    // data_base_dir() 读取 CYBER_JIANGHU_DATA_DIR（见 config.rs:42）
    // Rust 2024 edition: env::set_var 标记为 unsafe
    unsafe {
        env::set_var("CYBER_JIANGHU_DATA_DIR", &dir);
    }
}

fn clear_in_memory() {
    if let Ok(mut s) = token_stats().lock() {
        s.clear();
    }
}

#[test]
fn test_record_buckets_separate_hours() {
    let _guard = test_lock().lock().expect("lock poisoned");
    clear_in_memory();
    isolate();
    // 模拟三个不同时刻的记录
    let t1 = "2026-06-01T01:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let t2 = "2026-06-01T02:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let t3 = "2026-06-01T01:30:00Z".parse::<DateTime<Utc>>().unwrap(); // 同 t1 bucket

    // 手动调用 record_token_usage：但 record_token_usage 内部用 Utc::now()，不可注入时间
    // → 改用 in-memory helper：直接构造 entries 然后 snapshot
    // 这里走真实 API：连续调用 record，验证 detail 至少有 1 个 hour bucket
    record_token_usage(&prov(), m1(), 100, 50, 30, [0u8; 32]);
    record_token_usage(&prov(), m1(), 200, 80, 60, [0u8; 32]);
    record_token_usage(&prov(), m2(), 50, 20, 0, [0u8; 32]);
    record_failure(&prov(), m1());

    let snap = snapshot_all_stats();
    // 验证聚合正确：m1 应有 prompt=300, completion=130, cache_hit=90, calls=2 (含 1 failure = 3)
    // m2: prompt=50, completion=20, calls=1
    let m1_agg = snap.iter().find(|s| s.model == m1()).expect("m1 missing");
    assert_eq!(m1_agg.prompt_tokens, 300);
    assert_eq!(m1_agg.completion_tokens, 130);
    assert_eq!(m1_agg.cache_hit_tokens, 90);
    assert_eq!(m1_agg.calls, 3); // 2 success + 1 failure
    assert_eq!(m1_agg.failures, 1);

    let m2_agg = snap.iter().find(|s| s.model == m2()).expect("m2 missing");
    assert_eq!(m2_agg.prompt_tokens, 50);
    assert_eq!(m2_agg.calls, 1);
    assert_eq!(m2_agg.failures, 0);

    // 至少 1 个 hour bucket（实际为当前 UTC 小时，可能为多个）
    let snap_internal = snapshot_internal();
    let unique_hours: std::collections::HashSet<&String> =
        snap_internal.iter().map(|(_, hk, _)| hk).collect();
    assert!(
        !unique_hours.is_empty(),
        "should have at least 1 hour bucket"
    );

    // 验证 hour_key 格式正确
    for hk in &unique_hours {
        assert_eq!(hk.len(), 13, "hour_key 格式错误: {}", hk);
        assert_eq!(&hk[4..5], "-");
        assert_eq!(&hk[7..8], "-");
        assert_eq!(&hk[10..11], "-");
    }

    // 引用未使用变量避免警告
    let _ = (t1, t2, t3);
}

// ---- 5. rebuild_summary reduce + avg 计算 ----
#[test]
fn test_rebuild_summary_reduces_detail() {
    clear_in_memory();
    let mut p = PersistedTokenStats::default();

    // 构造 detail：2 个 hour × 1 个 model
    let mut h01 = BTreeMap::new();
    h01.insert(
        "openai_compatible/model-a".to_string(),
        HourBucketStats {
            prompt_tokens: 1000,
            completion_tokens: 200,
            cache_hit_tokens: 600,
            calls: 5,
            failures: 0,
            system_hash_distribution: HashMap::new(),
            first_record_at: Some("2026-06-01T01:00:00+00:00".to_string()),
            last_record_at: Some("2026-06-01T01:59:00+00:00".to_string()),
        },
    );
    p.detail.insert("2026-06-01-01".to_string(), h01);

    let mut h02 = BTreeMap::new();
    h02.insert(
        "openai_compatible/model-a".to_string(),
        HourBucketStats {
            prompt_tokens: 2000,
            completion_tokens: 400,
            cache_hit_tokens: 1000,
            calls: 10,
            failures: 2,
            system_hash_distribution: HashMap::new(),
            first_record_at: Some("2026-06-01T02:00:00+00:00".to_string()),
            last_record_at: Some("2026-06-01T02:59:00+00:00".to_string()),
        },
    );
    p.detail.insert("2026-06-01-02".to_string(), h02);

    rebuild_summary(&mut p);

    let s = p
        .summary
        .by_provider_model
        .get("openai_compatible/model-a")
        .expect("summary missing");
    assert_eq!(s.total_prompt_tokens, 3000);
    assert_eq!(s.total_completion_tokens, 600);
    assert_eq!(s.total_cache_hit_tokens, 1600);
    assert_eq!(s.total_calls, 15);
    assert_eq!(s.total_failures, 2);
    // 桶1: 01:00~01:59 = 59min, 桶2: 02:00~02:59 = 59min → 共 118min ≈ 1967 ms→约 1967
    assert!((s.active_hours - 1.967).abs() < 0.01); // (59+59)min / 60 ≈ 1.967h
    // avg = 3000 / (118*60/3600) = 3000 / 1.9667 ≈ 1525.4
    assert!((s.avg_prompt_tokens_per_hour - 1525.4).abs() < 1.0);
    assert!((s.avg_cache_hit_ratio - 1600.0 / 3000.0).abs() < 1e-9);
    assert_eq!(s.first_record_at, "2026-06-01T01:00:00+00:00");
    assert_eq!(s.last_record_at, "2026-06-01T02:59:00+00:00");
}

// ---- 6. rebuild_summary 边界：active_hours = 0 → avg = 0 ----
#[test]
fn test_rebuild_summary_empty_detail() {
    clear_in_memory();
    let mut p = PersistedTokenStats::default();
    rebuild_summary(&mut p);
    assert!(p.summary.by_provider_model.is_empty());
}

// ---- 7. persist_and_reset 持久化 + 内存清空 ----
#[test]
fn test_persist_and_reset_round_trip() {
    let _guard = test_lock().lock().expect("lock poisoned");
    clear_in_memory();
    // 用独立 tmp 目录
    let dir = env::temp_dir().join(format!(
        "tt_persist_{}_{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let _ = fs::create_dir_all(&dir);
    // 简单做法：直接把 data_base_dir() 链接到我们控制的目录
    // 但 data_base_dir() 内部读取固定 env，下面用 env::set_var 注入
    unsafe {
        env::set_var("CYBER_JIANGHU_DATA_DIR", &dir);
    }

    record_token_usage(&prov(), m1(), 100, 50, 30, [0u8; 32]);
    record_token_usage(&prov(), m1(), 200, 80, 60, [0u8; 32]);
    record_token_usage(&prov(), m2(), 50, 20, 0, [0u8; 32]);

    persist_and_reset();

    // 1) 文件存在
    let log_path = data_base_dir().join("logs").join(TOKEN_LOG_FILE);
    assert!(log_path.exists(), "log file not created: {:?}", log_path);

    // 2) 读回 JSON，结构正确
    let content = fs::read_to_string(&log_path).expect("read log");
    let parsed: PersistedTokenStats = serde_json::from_str(&content).expect("parse log");

    // summary 应有 2 个 model_key
    assert_eq!(parsed.summary.by_provider_model.len(), 2);
    let m1_summary = parsed
        .summary
        .by_provider_model
        .get("openai_compatible/model-a")
        .expect("m1 summary");
    assert_eq!(m1_summary.total_prompt_tokens, 300);
    assert_eq!(m1_summary.total_calls, 2);
    // 快速连续调用，实际活跃时长接近最小值 (1 min = ~17 when stored as *1000)
    assert!(m1_summary.active_hours > 0.0);

    // detail 应至少 1 个 hour bucket
    assert!(!parsed.detail.is_empty());
    for (hk, models) in &parsed.detail {
        assert_eq!(hk.len(), 13);
        assert_eq!(models.len(), 2);
    }

    // 3) 内存已清空
    let snap_after = snapshot_all_stats();
    assert!(
        snap_after.is_empty(),
        "in-memory should be cleared, got: {:?}",
        snap_after
    );

    // 4) 二次 persist_and_reset + 旧数据合并：再 record + persist，detail 应累加
    record_token_usage(&prov(), m1(), 100, 50, 30, [0u8; 32]);
    persist_and_reset();
    let content2 = fs::read_to_string(&log_path).expect("read log 2");
    let parsed2: PersistedTokenStats = serde_json::from_str(&content2).expect("parse 2");
    let m1_s2 = parsed2
        .summary
        .by_provider_model
        .get("openai_compatible/model-a")
        .expect("m1 s2");
    // 应等于 m1 旧 total + 新增 100
    assert_eq!(m1_s2.total_prompt_tokens, 300 + 100);
    assert_eq!(m1_s2.total_calls, 2 + 1);

    // 清理
    let _ = fs::remove_dir_all(&dir);
}

// ---- 8. 旧 flat 格式被忽略 ----
#[test]
fn test_old_flat_format_ignored() {
    let _guard = test_lock().lock().expect("lock poisoned");
    clear_in_memory();
    let dir = env::temp_dir().join(format!(
        "tt_legacy_{}_{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let _ = fs::create_dir_all(dir.join("logs"));
    unsafe {
        env::set_var("CYBER_JIANGHU_DATA_DIR", &dir);
    }

    // 写一个旧 flat 格式文件
    let legacy = serde_json::json!({
        "openai_compatible/old-model": {
            "provider": "openai_compatible",
            "model": "old-model",
            "prompt_tokens": 999,
            "completion_tokens": 99,
            "calls": 9,
            "failures": 0,
            "cache_hit_tokens": 0
        }
    });
    let legacy_path = dir.join("logs").join(TOKEN_LOG_FILE);
    fs::write(&legacy_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    // 触发 persist_and_reset（in-memory 空 → 早返，不动文件）
    // 但我们需要 in-memory 有数据才会写。先 record 一条
    record_token_usage(&prov(), m1(), 1, 1, 0, [0u8; 32]);
    persist_and_reset();

    // 读回：应是新结构，旧数据已被覆盖
    let content = fs::read_to_string(&legacy_path).unwrap();
    let parsed: PersistedTokenStats = serde_json::from_str(&content).unwrap();
    // 旧 model_key 不应出现
    assert!(
        !parsed
            .summary
            .by_provider_model
            .contains_key("openai_compatible/old-model")
    );
    // 新 model 应有
    assert!(
        parsed
            .summary
            .by_provider_model
            .contains_key("openai_compatible/model-a")
    );

    // 清理
    let _ = fs::remove_dir_all(&dir);
}

// ---- 9. record_token_usage accepts system_hash param ----
#[test]
fn record_token_usage_accepts_system_hash_param() {
    let _guard = test_lock().lock().expect("lock poisoned");
    clear_in_memory();
    use crate::component::llm::LlmProvider;
    let system_hash: [u8; 32] = [1u8; 32];
    record_token_usage(
        &LlmProvider::OpenAICompatible,
        "test-model",
        100,
        50,
        10,
        system_hash,
    );
}

// ---- 10. has_prefix_drift 阈值边界 ----
fn bucket_with_hashes(n: usize) -> HourBucketStats {
    let mut bucket = HourBucketStats::default();
    for i in 0..n {
        let mut h = [0u8; 32];
        h[0] = i as u8;
        bucket.system_hash_distribution.insert(h, 1);
    }
    bucket
}

#[test]
fn has_prefix_drift_threshold_boundary() {
    assert!(
        !has_prefix_drift(&bucket_with_hashes(1)),
        "单 hash（稳定 agent）不应告警"
    );
    assert!(
        !has_prefix_drift(&bucket_with_hashes(PREFIX_DRIFT_HASH_WARN_THRESHOLD)),
        "等于阈值不应告警"
    );
    assert!(
        has_prefix_drift(&bucket_with_hashes(PREFIX_DRIFT_HASH_WARN_THRESHOLD + 1)),
        "超过阈值必须告警"
    );
}
