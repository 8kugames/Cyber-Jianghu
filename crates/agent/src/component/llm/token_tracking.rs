// ============================================================================
// Token Usage Tracking (per provider-model + per-component)
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::direct_client::LlmProvider;

// ============================================================================
// Per-Component Token Metrics (Phase 0a Instrumentation)
// ============================================================================

/// LLM 调用的组件标签 — 用于追踪每个组件的 token 消耗
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LlmComponent {
    CognitiveEngine,
    AttentionController,
    ReflectorLayer3,
    ToolCalling,
    SocialProcessing,
}

/// 单个组件的 token 聚合指标
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComponentMetrics {
    pub call_count: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}

static COMPONENT_STATS: OnceLock<Mutex<HashMap<LlmComponent, ComponentMetrics>>> = OnceLock::new();

fn component_stats() -> &'static Mutex<HashMap<LlmComponent, ComponentMetrics>> {
    COMPONENT_STATS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 记录带组件标签的 token 使用量（不破坏现有 record_token_usage 签名）
pub fn record_token_usage_with_component(
    provider: &LlmProvider,
    model: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
    component: LlmComponent,
) {
    // 同时记录到 per-model 统计
    record_token_usage(provider, model, prompt_tokens, completion_tokens);

    // 记录到 per-component 统计
    if let Ok(mut stats) = component_stats().lock() {
        let entry = stats.entry(component).or_default();
        entry.call_count += 1;
        entry.total_input_tokens += prompt_tokens;
        entry.total_output_tokens += completion_tokens;
    }
}

/// 获取所有组件指标的快照（不清除）
pub fn snapshot_component_stats() -> HashMap<LlmComponent, ComponentMetrics> {
    match component_stats().lock() {
        Ok(stats) => stats.clone(),
        Err(_) => HashMap::new(),
    }
}

/// Per-model token stats
struct PerModelStats {
    prompt_tokens: u64,
    completion_tokens: u64,
    calls: u64,
    failures: u64,
}

impl PerModelStats {
    fn new() -> Self {
        Self {
            prompt_tokens: 0,
            completion_tokens: 0,
            calls: 0,
            failures: 0,
        }
    }

    fn record(&mut self, prompt: u64, completion: u64) {
        self.prompt_tokens += prompt;
        self.completion_tokens += completion;
        self.calls += 1;
    }
}

/// Token stats for a specific provider-model key
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTokenStats {
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(skip)]
    pub total_tokens: u64, // 仅在聚合时使用，不序列化
    pub calls: u64,
    #[serde(default)]
    pub failures: u64,
}

static TOKEN_STATS: OnceLock<Mutex<HashMap<String, PerModelStats>>> = OnceLock::new();

fn token_stats() -> &'static Mutex<HashMap<String, PerModelStats>> {
    TOKEN_STATS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn model_key(provider: &LlmProvider, model: &str) -> String {
    format!("{}/{}", provider.as_str(), model)
}

const TOKEN_LOG_FILE: &str = "token_cost_count.tmp";

fn log_file_path() -> Option<PathBuf> {
    // 优先使用 CYBER_JIANGHU_DATA_DIR（Docker 挂载，容器重启后持久化）
    let data_dir = std::env::var("CYBER_JIANGHU_DATA_DIR")
        .ok()
        .map(PathBuf::from);
    let log_dir = data_dir
        .or_else(|| dirs::home_dir().map(|h| h.join(".cyber-jianghu")))
        .map(|d| d.join("logs"));
    log_dir.map(|d| d.join(TOKEN_LOG_FILE))
}

/// Record token usage for a specific provider-model
pub fn record_token_usage(
    provider: &LlmProvider,
    model: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
) {
    let key = model_key(provider, model);
    if let Ok(mut stats) = token_stats().lock() {
        stats
            .entry(key)
            .or_insert_with(PerModelStats::new)
            .record(prompt_tokens, completion_tokens);
    }
}

/// Record a failed LLM call for a specific provider-model
pub fn record_failure(provider: &LlmProvider, model: &str) {
    let key = model_key(provider, model);
    if let Ok(mut stats) = token_stats().lock() {
        let entry = stats.entry(key).or_insert_with(PerModelStats::new);
        entry.calls += 1;
        entry.failures += 1;
    }
}

/// Get snapshot of all model stats (does not clear)
pub fn snapshot_all_stats() -> Vec<ModelTokenStats> {
    let Ok(stats) = token_stats().lock() else {
        return vec![];
    };
    stats
        .iter()
        .map(|(key, s)| {
            let parts: Vec<&str> = key.splitn(2, '/').collect();
            let (provider, model) = if parts.len() == 2 {
                (parts[0].to_string(), parts[1].to_string())
            } else {
                ("unknown".to_string(), key.clone())
            };
            let total = s.prompt_tokens + s.completion_tokens;
            ModelTokenStats {
                provider,
                model,
                prompt_tokens: s.prompt_tokens,
                completion_tokens: s.completion_tokens,
                total_tokens: total,
                calls: s.calls,
                failures: s.failures,
            }
        })
        .collect()
}

/// Persist all stats to file and reset counters
pub fn persist_and_reset() {
    let stats = snapshot_all_stats();
    if stats.is_empty() {
        return;
    }
    if let Some(path) = log_file_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // Read existing data
        let existing: HashMap<String, ModelTokenStats> = if path.exists() {
            let content = fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };
        // Merge: add to existing counts
        let mut merged: HashMap<String, ModelTokenStats> = existing;
        for s in &stats {
            let key = format!("{}/{}", s.provider, s.model);
            if let Some(existing) = merged.get_mut(&key) {
                existing.prompt_tokens += s.prompt_tokens;
                existing.completion_tokens += s.completion_tokens;
                existing.total_tokens += s.prompt_tokens + s.completion_tokens;
                existing.calls += s.calls;
                existing.failures += s.failures;
            } else {
                merged.insert(key, s.clone());
            }
        }
        // Write back (atomic: write to tmp file then rename)
        if let Ok(json) = serde_json::to_string_pretty(&merged) {
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
    }
    // Reset current tick counters
    if let Ok(mut stats) = token_stats().lock() {
        stats.clear();
    }
}
