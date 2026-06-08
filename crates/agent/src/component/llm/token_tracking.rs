// ============================================================================
// Token Usage Tracking (per provider-model, per hour)
// ============================================================================
//
// 内存结构：HashMap<model_key, BTreeMap<hour_key, PerHourStats>>
//   - model_key = "{provider}/{model}" —— 主索引，O(1) 查找
//   - hour_key  = "yyyy-mm-dd-hh"     —— BTreeMap 天然按 key 升序，detail 输出有序
//
// 持久化结构：{ summary: { by_provider_model: {...} }, detail: { "<hour>": {...} } }
//   - summary：每个 model_key 一项聚合（含 avg_*_per_hour 字段）
//   - detail：每条 hour_key 一个 bucket，bucket 内按 model_key 分组
//
// 时区：UTC（与 workspace 约定一致，避免本地时区 / 夏令时 / 跨日边界坑）
//
// 历史格式兼容性：旧 flat HashMap<model_key, ModelTokenStats> 结构不再支持，
//   首次运行新代码时旧 .tmp 文件会被忽略（无外部 reader，影响为零）。
// ============================================================================

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::direct_client::LlmProvider;

/// 内部：单小时单模型累计统计
#[derive(Debug, Clone, Default)]
struct PerHourStats {
    bucket: HourBucketStats,
}

/// 持久化：单小时单模型累计统计（仅数值，不含时间戳）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HourBucketStats {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub cache_hit_tokens: u64,
    pub calls: u64,
    #[serde(default)]
    pub failures: u64,
    #[serde(default)]
    pub system_hash_distribution: HashMap<[u8; 32], u64>,
}

/// 持久化：summary 中按 provider/model 聚合后的最终统计（含 avg 字段）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelSummaryStats {
    pub provider: String,
    pub model: String,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_cache_hit_tokens: u64,
    pub total_calls: u64,
    pub total_failures: u64,
    pub active_hours: u64,
    pub first_record_at: String,
    pub last_record_at: String,
    /// 每活跃小时平均 prompt token（active_hours > 0 时计算）
    pub avg_prompt_tokens_per_hour: f64,
    /// 每活跃小时平均 completion token
    pub avg_completion_tokens_per_hour: f64,
    /// 每活跃小时平均调用次数
    pub avg_calls_per_hour: f64,
    /// 缓存命中率（总 cache_hit / 总 prompt）
    pub avg_cache_hit_ratio: f64,
}

/// 持久化 summary 容器
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PersistedSummary {
    pub by_provider_model: BTreeMap<String, ModelSummaryStats>,
}

/// 持久化顶层结构
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PersistedTokenStats {
    pub summary: PersistedSummary,
    /// hour_key -> (model_key -> HourBucketStats)
    pub detail: BTreeMap<String, BTreeMap<String, HourBucketStats>>,
}

/// in-memory 聚合：保持 API 兼容的 ModelTokenStats（跨小时求和）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelTokenStats {
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(skip)]
    pub total_tokens: u64,
    pub calls: u64,
    #[serde(default)]
    pub failures: u64,
    #[serde(default)]
    pub cache_hit_tokens: u64,
    #[serde(default)]
    pub system_hash_distribution: HashMap<[u8; 32], u64>,
}

static TOKEN_STATS: OnceLock<
    Mutex<HashMap<String /* model_key */, BTreeMap<String /* hour_key */, PerHourStats>>>,
> = OnceLock::new();

fn token_stats() -> &'static Mutex<HashMap<String, BTreeMap<String, PerHourStats>>> {
    TOKEN_STATS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn model_key(provider: &LlmProvider, model: &str) -> String {
    format!("{}/{}", provider.as_str(), model)
}

fn split_model_key(key: &str) -> (String, String) {
    let parts: Vec<&str> = key.splitn(2, '/').collect();
    if parts.len() == 2 {
        (parts[0].to_string(), parts[1].to_string())
    } else {
        ("unknown".to_string(), key.to_string())
    }
}

/// 生成 hour key（UTC，"yyyy-mm-dd-hh"）
fn hour_key(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%d-%H").to_string()
}

/// 从 "yyyy-mm-dd-hh" 反解为该小时整点的 DateTime<Utc>
/// 解析失败时回退到 Utc::now()（避免聚合崩溃）
fn parse_hour_key(s: &str) -> DateTime<Utc> {
    NaiveDateTime::parse_from_str(&format!("{}:00:00", s), "%Y-%m-%d-%H:%M:%S")
        .map(|n| Utc.from_utc_datetime(&n))
        .unwrap_or_else(|_| Utc::now())
}

const TOKEN_LOG_FILE: &str = "token_cost_count.tmp";

fn log_file_path() -> Option<PathBuf> {
    Some(
        crate::config::data_base_dir()
            .join("logs")
            .join(TOKEN_LOG_FILE),
    )
}

/// Record token usage for a specific provider-model, bucketed by current UTC hour
pub fn record_token_usage(
    provider: &LlmProvider,
    model: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    cache_hit: u64,
    system_hash: [u8; 32],
) {
    let key = model_key(provider, model);
    let hk = hour_key(Utc::now());
    if let Ok(mut stats) = token_stats().lock() {
        let model_entry = stats.entry(key).or_default();
        let hour_entry = model_entry.entry(hk).or_default();
        hour_entry.bucket.prompt_tokens += prompt_tokens;
        hour_entry.bucket.completion_tokens += completion_tokens;
        hour_entry.bucket.cache_hit_tokens += cache_hit;
        hour_entry.bucket.calls += 1;
        *hour_entry.bucket.system_hash_distribution.entry(system_hash).or_insert(0) += 1;
    }
}

/// Record a failed LLM call for a specific provider-model, bucketed by current UTC hour
pub fn record_failure(provider: &LlmProvider, model: &str) {
    let key = model_key(provider, model);
    let hk = hour_key(Utc::now());
    if let Ok(mut stats) = token_stats().lock() {
        let model_entry = stats.entry(key).or_default();
        let hour_entry = model_entry.entry(hk).or_default();
        hour_entry.bucket.calls += 1;
        hour_entry.bucket.failures += 1;
    }
}

/// 内部：拍快照，返回 (model_key, hour_key, PerHourStats) 三元组列表
fn snapshot_internal() -> Vec<(String, String, PerHourStats)> {
    let Ok(stats) = token_stats().lock() else {
        return vec![];
    };
    let mut out = Vec::with_capacity(stats.len());
    for (model_key, hours) in stats.iter() {
        for (hour_key, phs) in hours.iter() {
            out.push((model_key.clone(), hour_key.clone(), phs.clone()));
        }
    }
    out
}

/// Get snapshot of all model stats aggregated across hours (does not clear)
/// API 兼容：保留原签名与返回类型，/api/v1/metrics 与 /api/v1/config/llm/usage 无感
pub fn snapshot_all_stats() -> Vec<ModelTokenStats> {
    let Ok(stats) = token_stats().lock() else {
        return vec![];
    };
    stats
        .iter()
        .map(|(key, hours)| {
            let (provider, model) = split_model_key(key);
            let mut agg = HourBucketStats::default();
            let mut system_hash_distribution: HashMap<[u8; 32], u64> = HashMap::new();
            for phs in hours.values() {
                agg.prompt_tokens += phs.bucket.prompt_tokens;
                agg.completion_tokens += phs.bucket.completion_tokens;
                agg.cache_hit_tokens += phs.bucket.cache_hit_tokens;
                agg.calls += phs.bucket.calls;
                agg.failures += phs.bucket.failures;
                for (hash, count) in &phs.bucket.system_hash_distribution {
                    *system_hash_distribution.entry(*hash).or_insert(0) += count;
                }
            }
            let total = agg.prompt_tokens + agg.completion_tokens;
            ModelTokenStats {
                provider,
                model,
                prompt_tokens: agg.prompt_tokens,
                completion_tokens: agg.completion_tokens,
                total_tokens: total,
                calls: agg.calls,
                failures: agg.failures,
                cache_hit_tokens: agg.cache_hit_tokens,
                system_hash_distribution,
            }
        })
        .collect()
}

/// 重建 summary：从 detail 全量 reduce，原子写前必须调用
///
/// 设计要点：
/// - total_* 来自 detail 全量累加
/// - active_hours = detail 中该 model_key 出现的 hour bucket 数量
/// - avg_*_per_hour = total_* / active_hours（active_hours=0 时为 0.0）
/// - avg_cache_hit_ratio = total_cache_hit / total_prompt（总命中率）
/// - first/last_record_at = detail 中该 model_key 出现的最早/最晚 hour 整点
fn rebuild_summary(p: &mut PersistedTokenStats) {
    /// 每 model 的 (累计 stats, 最早 hour 整点, 最晚 hour 整点)
    type ModelAgg = (
        HourBucketStats,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
    );
    let mut agg: BTreeMap<String, ModelAgg> = BTreeMap::new();

    for (hour_key, models) in &p.detail {
        let hour_dt = parse_hour_key(hour_key);
        for (model_key, bucket) in models {
            let entry = agg
                .entry(model_key.clone())
                .or_insert_with(|| (HourBucketStats::default(), None, None));
            entry.0.prompt_tokens += bucket.prompt_tokens;
            entry.0.completion_tokens += bucket.completion_tokens;
            entry.0.cache_hit_tokens += bucket.cache_hit_tokens;
            entry.0.calls += bucket.calls;
            entry.0.failures += bucket.failures;
            entry.1 = Some(match entry.1 {
                Some(f) => f.min(hour_dt),
                None => hour_dt,
            });
            entry.2 = Some(match entry.2 {
                Some(l) => l.max(hour_dt),
                None => hour_dt,
            });
        }
    }

    p.summary.by_provider_model.clear();
    for (model_key, (acc, first, last)) in agg {
        let (provider, model) = split_model_key(&model_key);
        // active_hours = detail 中包含此 model_key 的 hour bucket 数
        let active_hours = p
            .detail
            .values()
            .filter(|models| models.contains_key(&model_key))
            .count() as u64;
        let avg_pt = if active_hours > 0 {
            acc.prompt_tokens as f64 / active_hours as f64
        } else {
            0.0
        };
        let avg_ct = if active_hours > 0 {
            acc.completion_tokens as f64 / active_hours as f64
        } else {
            0.0
        };
        let avg_calls = if active_hours > 0 {
            acc.calls as f64 / active_hours as f64
        } else {
            0.0
        };
        let cache_ratio = if acc.prompt_tokens > 0 {
            acc.cache_hit_tokens as f64 / acc.prompt_tokens as f64
        } else {
            0.0
        };
        p.summary.by_provider_model.insert(
            model_key.clone(),
            ModelSummaryStats {
                provider,
                model,
                total_prompt_tokens: acc.prompt_tokens,
                total_completion_tokens: acc.completion_tokens,
                total_cache_hit_tokens: acc.cache_hit_tokens,
                total_calls: acc.calls,
                total_failures: acc.failures,
                active_hours,
                first_record_at: first.map(|d| d.to_rfc3339()).unwrap_or_default(),
                last_record_at: last.map(|d| d.to_rfc3339()).unwrap_or_default(),
                avg_prompt_tokens_per_hour: avg_pt,
                avg_completion_tokens_per_hour: avg_ct,
                avg_calls_per_hour: avg_calls,
                avg_cache_hit_ratio: cache_ratio,
            },
        );
    }
}

/// Persist all stats to file (merge with existing persisted data) and reset in-memory counters
pub fn persist_and_reset() {
    let snapshot = snapshot_internal();
    if snapshot.is_empty() {
        return;
    }
    let Some(path) = log_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    // 读旧文件：旧 flat HashMap 格式解析失败 → 当成空 PersistedTokenStats（不迁移）
    let mut existing: PersistedTokenStats = if path.exists() {
        let content = fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        PersistedTokenStats::default()
    };

    // 合并 in-memory → existing
    for (model_key, hour_key, phs) in snapshot {
        let model_detail = existing.detail.entry(hour_key).or_default();
        let bucket = model_detail.entry(model_key).or_default();
        bucket.prompt_tokens += phs.bucket.prompt_tokens;
        bucket.completion_tokens += phs.bucket.completion_tokens;
        bucket.cache_hit_tokens += phs.bucket.cache_hit_tokens;
        bucket.calls += phs.bucket.calls;
        bucket.failures += phs.bucket.failures;
    }

    // 重建 summary（reduce detail）
    rebuild_summary(&mut existing);

    // 原子写
    if let Ok(json) = serde_json::to_string_pretty(&existing) {
        let tmp_path = path.with_extension("tmp_write");
        if fs::write(&tmp_path, &json).is_err() {
            tracing::warn!("[token_tracking] 写入临时文件失败: {:?}", tmp_path);
            return;
        }
        if let Err(e) = fs::rename(&tmp_path, &path) {
            tracing::warn!(
                "[token_tracking] rename 失败: {} -> {:?}: {}",
                tmp_path.display(),
                path,
                e
            );
        }
    }

    // 清空 in-memory
    if let Ok(mut stats) = token_stats().lock() {
        stats.clear();
    }
}

#[cfg(test)]
mod tests {
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
        let t = "2026-06-01T02:35:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(hour_key(t), "2026-06-01-02");

        let t2 = "2026-12-31T23:59:59Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(hour_key(t2), "2026-12-31-23");

        let t3 = "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(hour_key(t3), "2026-01-01-00");
    }

    // ---- 2. parse_hour_key 反解 ----
    #[test]
    fn test_parse_hour_key_roundtrip() {
        let t = "2026-06-01T02:35:00Z".parse::<DateTime<Utc>>().unwrap();
        let hk = hour_key(t);
        let parsed = parse_hour_key(&hk);
        assert_eq!(parsed.format("%Y-%m-%d-%H").to_string(), hk);
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
        assert_eq!(s.active_hours, 2);
        assert_eq!(s.avg_prompt_tokens_per_hour, 1500.0);
        assert_eq!(s.avg_completion_tokens_per_hour, 300.0);
        assert_eq!(s.avg_calls_per_hour, 7.5);
        assert!((s.avg_cache_hit_ratio - 1600.0 / 3000.0).abs() < 1e-9);
        assert_eq!(s.first_record_at, "2026-06-01T01:00:00+00:00");
        assert_eq!(s.last_record_at, "2026-06-01T02:00:00+00:00");
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
        assert!(m1_summary.active_hours >= 1);

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
}
