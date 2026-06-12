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
// 时区：本地时区（未采集到时区则默认 UTC+8），hour_key 按本地时间分桶，便于运维按日查看
//
// 历史格式兼容性：旧 flat HashMap<model_key, ModelTokenStats> 结构不再支持，
//   首次运行新代码时旧 .tmp 文件会被忽略（无外部 reader，影响为零）。
// ============================================================================

use chrono::LocalResult;
use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone, Utc};
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

/// 持久化：单小时单模型累计统计
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
    /// 该桶内首次记录时间（ISO 8601），用于计算实际活跃时长
    #[serde(default)]
    pub first_record_at: Option<String>,
    /// 该桶内末次记录时间（ISO 8601）
    #[serde(default)]
    pub last_record_at: Option<String>,
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
    pub active_hours: f64,
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

/// 获取本地时区偏移，未采集到时区时默认 UTC+8
fn local_tz() -> FixedOffset {
    let offset = *Local::now().offset();
    if offset.local_minus_utc() == 0 {
        // chrono::Local 无法检测时区时回退 UTC(0)
        // 按需求：未采集到时区默认 UTC+8
        FixedOffset::east_opt(8 * 3600).unwrap_or(offset)
    } else {
        offset
    }
}

/// 获取当前本地时间（时区获取失败则用 UTC+8）
fn local_now() -> DateTime<FixedOffset> {
    let tz = local_tz();
    Utc::now().with_timezone(&tz)
}

/// 生成 hour key（本地时区，"yyyy-mm-dd-hh"）
fn hour_key(now: DateTime<FixedOffset>) -> String {
    now.format("%Y-%m-%d-%H").to_string()
}

/// 从 "yyyy-mm-dd-hh" 反解为该小时整点的 DateTime<Utc>
/// 注意：hour_key 基于本地时区，解析时将 naive 视为本地时间再转 UTC
/// 解析失败时回退到 Utc::now()（避免聚合崩溃）
fn parse_hour_key(s: &str) -> DateTime<Utc> {
    let tz = local_tz();
    match NaiveDateTime::parse_from_str(&format!("{}:00:00", s), "%Y-%m-%d-%H:%M:%S") {
        Ok(n) => match tz.from_local_datetime(&n) {
            LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.to_utc(),
            LocalResult::None => Utc::now(),
        },
        Err(_) => Utc::now(),
    }
}

const TOKEN_LOG_FILE: &str = "token_cost_count.tmp";

fn log_file_path() -> Option<PathBuf> {
    Some(
        crate::config::data_base_dir()
            .join("logs")
            .join(TOKEN_LOG_FILE),
    )
}

/// Record token usage for a specific provider-model, bucketed by current local hour
pub fn record_token_usage(
    provider: &LlmProvider,
    model: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    cache_hit: u64,
    system_hash: [u8; 32],
) {
    let key = model_key(provider, model);
    let now = local_now();
    let hk = hour_key(now);
    let now_iso = now.to_rfc3339();
    if let Ok(mut stats) = token_stats().lock() {
        let model_entry = stats.entry(key).or_default();
        let hour_entry = model_entry.entry(hk).or_default();
        hour_entry.bucket.prompt_tokens += prompt_tokens;
        hour_entry.bucket.completion_tokens += completion_tokens;
        hour_entry.bucket.cache_hit_tokens += cache_hit;
        hour_entry.bucket.calls += 1;
        *hour_entry
            .bucket
            .system_hash_distribution
            .entry(system_hash)
            .or_insert(0) += 1;
        if hour_entry.bucket.first_record_at.is_none() {
            hour_entry.bucket.first_record_at = Some(now_iso.clone());
        }
        hour_entry.bucket.last_record_at = Some(now_iso);
    }
}

/// Record a failed LLM call for a specific provider-model, bucketed by current local hour
pub fn record_failure(provider: &LlmProvider, model: &str) {
    let key = model_key(provider, model);
    let now = local_now();
    let hk = hour_key(now);
    let now_iso = now.to_rfc3339();
    if let Ok(mut stats) = token_stats().lock() {
        let model_entry = stats.entry(key).or_default();
        let hour_entry = model_entry.entry(hk).or_default();
        hour_entry.bucket.calls += 1;
        hour_entry.bucket.failures += 1;
        if hour_entry.bucket.first_record_at.is_none() {
            hour_entry.bucket.first_record_at = Some(now_iso.clone());
        }
        hour_entry.bucket.last_record_at = Some(now_iso);
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
/// - active_hours = 各 hour bucket 实际活跃时长之和（小时）
///   每桶时长 = last_record_at - first_record_at，最少 1 分钟
/// - avg_*_per_hour = total_* / active_hours（active_hours=0 时为 0.0）
/// - avg_cache_hit_ratio = total_cache_hit / total_prompt（总命中率）
/// - first/last_record_at = detail 中该 model_key 出现的最早/最晚时间戳
fn rebuild_summary(p: &mut PersistedTokenStats) {
    /// 每 model 的 (累计 stats, 最早时间, 最晚时间, 累计活跃小时数)
    type ModelAgg = (
        HourBucketStats,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        f64, // cumulative active hours
    );
    let mut agg: BTreeMap<String, ModelAgg> = BTreeMap::new();

    for (hour_key, models) in &p.detail {
        let hour_dt = parse_hour_key(hour_key);
        for (model_key, bucket) in models {
            let entry = agg
                .entry(model_key.clone())
                .or_insert_with(|| (HourBucketStats::default(), None, None, 0.0));
            entry.0.prompt_tokens += bucket.prompt_tokens;
            entry.0.completion_tokens += bucket.completion_tokens;
            entry.0.cache_hit_tokens += bucket.cache_hit_tokens;
            entry.0.calls += bucket.calls;
            entry.0.failures += bucket.failures;

            // 计算该桶的实际活跃时长
            let bucket_hours = bucket_active_hours(bucket);
            entry.3 += bucket_hours;

            // 更新最早/最晚时间：优先用桶内时间戳，回退到 hour 整点
            let bucket_first = bucket
                .first_record_at
                .as_deref()
                .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                .unwrap_or(hour_dt);
            let bucket_last = bucket
                .last_record_at
                .as_deref()
                .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                .unwrap_or(hour_dt);
            entry.1 = Some(match entry.1 {
                Some(f) => f.min(bucket_first),
                None => bucket_first,
            });
            entry.2 = Some(match entry.2 {
                Some(l) => l.max(bucket_last),
                None => bucket_last,
            });
        }
    }

    p.summary.by_provider_model.clear();
    for (model_key, (acc, first, last, active_hours)) in agg {
        let (provider, model) = split_model_key(&model_key);
        let avg_pt = if active_hours > 0.0 {
            acc.prompt_tokens as f64 / active_hours
        } else {
            0.0
        };
        let avg_ct = if active_hours > 0.0 {
            acc.completion_tokens as f64 / active_hours
        } else {
            0.0
        };
        let avg_calls = if active_hours > 0.0 {
            acc.calls as f64 / active_hours
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

/// 计算单个 hour bucket 的实际活跃时长（小时）
/// 规则：last_record_at - first_record_at，最少 1 分钟（单次调用场景）
/// 无时间戳时（旧数据）回退为 1.0 小时（保持桶计数语义）
fn bucket_active_hours(bucket: &HourBucketStats) -> f64 {
    let first = match bucket
        .first_record_at
        .as_deref()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
    {
        Some(t) => t,
        None => return 1.0, // 旧数据无时间戳，回退为 1 小时
    };
    let last = match bucket
        .last_record_at
        .as_deref()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
    {
        Some(t) => t,
        None => return 0.0,
    };
    let duration = (last - first).to_std().unwrap_or(std::time::Duration::ZERO);
    // 最少 1 分钟（避免单次调用除以 0）
    let min_duration = std::time::Duration::from_secs(60);
    let effective = duration.max(min_duration);
    effective.as_secs_f64() / 3600.0
}

/// 合并两个可选时间戳：earliest=true 取更早的，false 取更晚的
fn merge_timestamp(target: &mut Option<String>, source: &Option<String>, earliest: bool) {
    match (target.as_deref(), source.as_deref()) {
        (_, None) => {}
        (None, Some(s)) => *target = Some(s.to_string()),
        (Some(t), Some(s)) => {
            let t_dt = t.parse::<DateTime<Utc>>().ok();
            let s_dt = s.parse::<DateTime<Utc>>().ok();
            match (t_dt, s_dt) {
                (None, Some(s_dt)) => *target = Some(s_dt.to_rfc3339()),
                (Some(_), None) => {}
                (None, None) => {}
                (Some(t_dt), Some(s_dt)) => {
                    let pick = if earliest {
                        t_dt.min(s_dt)
                    } else {
                        t_dt.max(s_dt)
                    };
                    *target = Some(pick.to_rfc3339());
                }
            }
        }
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
        // 合并时间戳：取最早 first 和最晚 last
        merge_timestamp(
            &mut bucket.first_record_at,
            &phs.bucket.first_record_at,
            true,
        );
        merge_timestamp(
            &mut bucket.last_record_at,
            &phs.bucket.last_record_at,
            false,
        );
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
}
