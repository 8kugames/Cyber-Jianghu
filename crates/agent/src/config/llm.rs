// ============================================================================
// LLM 配置（LlmConfig / FallbackModelConfig / CacheDiagnosticsConfig）
// ============================================================================

use super::*;

// ============================================================================
// ============================================================================
// LLM 配置（仅 Cognitive 模式使用）
// ============================================================================

// 所有 LLM/agent 相关默认常量从 protocol crate 引入,避免重复定义。
// 单一来源原则: 改默认值仅需改 protocol/src/lib.rs 一处。
/// 驳回反馈跨 tick 保留时长（tick）：仅对紧邻的下一 tick 决策可见，
/// 防止过时反馈滞留导致行为过度抑制（如环境已变化仍不敢行动）。
/// agent 本地决策常量，非 wire 契约默认值，故定义于 agent crate 而非 protocol。
pub(crate) const REJECTION_FEEDBACK_TTL_TICKS: i64 = 1;

/// 上轮天魂驳回记录的显示窗口（age = 当前 tick - 记录 tick）。
/// 常规路径 soul_cycle 每 tick 重写，实际显示仅最近 1 tick；
/// 上界 2 为防御性余量（容忍连续决策被跳过的异常路径）。
/// agent 本地决策常量，非 wire 契约默认值。
pub(crate) const REJECTION_RECORD_TTL_TICKS: i64 = 2;

pub(crate) use cyber_jianghu_protocol::{
    DEFAULT_CONTEXT_WINDOW_TOKENS, DEFAULT_ENABLE_STREAMING, DEFAULT_EXECUTION_RESULT_TIMEOUT_MS,
    DEFAULT_IDLE_ROTATE_THRESHOLD, DEFAULT_KEEP_RECENT_TURNS, DEFAULT_LLM_CONNECT_TIMEOUT_SECS,
    DEFAULT_LLM_MAX_TOKENS, DEFAULT_LLM_PROVIDER, DEFAULT_LLM_REQUEST_TIMEOUT_SECS,
    DEFAULT_LLM_TEMPERATURE, DEFAULT_NARRATIVE_WINDOW_SIZE, DEFAULT_RECONNECT_DELAY_SECS,
    DEFAULT_SEMANTIC_DEDUP_HISTORY, DEFAULT_SOUL_CYCLE_REPORT_BASE_DELAY_MS,
    DEFAULT_SOUL_CYCLE_REPORT_RETRIES, DEFAULT_SUMMARY_TRIGGER_RATIO,
};

/// 单个模型的独立配置（允许 per-model max_tokens）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackModelConfig {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// DashScope/Kimi 等模型的 enable_thinking 参数（None = 不发送该字段）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
    /// 模型上下文窗口大小（None = 使用全局 context_window_tokens）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default = "default_llm_temperature")]
    pub temperature: f32,
    #[serde(default = "default_llm_max_tokens")]
    pub max_tokens: u32,
    /// 备用模型列表（同 provider/api_key，主模型 403/超时时自动降级）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_models: Vec<String>,
    /// 模型独立配置列表（优先于 fallback_models，允许 per-model max_tokens）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<FallbackModelConfig>,
    /// 连续 idle tick 数达到此阈值后主动切换到下一个模型
    #[serde(default = "default_idle_rotate_threshold")]
    pub idle_rotate_threshold: u32,

    /// 上下文窗口 token 数（用于长窗口对话）
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: u32,

    /// Summary 触发比例 (0.0 - 1.0)，token 数超过此比例时触发压缩
    #[serde(default = "default_summary_trigger_ratio")]
    pub summary_trigger_ratio: f64,

    /// Summary 后保留最近 N 轮对话
    #[serde(default = "default_keep_recent_turns")]
    pub keep_recent_turns: u32,

    /// 重连延迟（秒）
    #[serde(default = "default_reconnect_delay_secs")]
    pub reconnect_delay_secs: u64,

    /// 等待执行结果超时（毫秒）
    #[serde(default = "default_execution_result_timeout_ms")]
    pub execution_result_timeout_ms: u64,

    /// 灵魂周期上报重试次数
    #[serde(default = "default_soul_cycle_report_retries")]
    pub soul_cycle_report_retries: u32,

    /// 灵魂周期上报基础延迟（毫秒），指数退避
    #[serde(default = "default_soul_cycle_report_base_delay_ms")]
    pub soul_cycle_report_base_delay_ms: u64,

    /// NarrativeSummaryWindow 窗口大小
    #[serde(default = "default_narrative_window_size")]
    pub narrative_window_size: usize,

    /// 启用 SSE 流式 LLM 调用（减少有效延迟）
    #[serde(default = "default_enable_streaming")]
    pub enable_streaming: bool,

    /// DashScope/Kimi 等模型的 enable_thinking 参数（None = 不发送该字段）
    /// per-model 配置优先于此全局值
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,

    /// LLM HTTP 请求整体超时（秒）。Agent 端 LLM 调用最坏耗时 = `max_retries × request_timeout_secs`，
    /// 默认 120s（与 Server LlmConfig 对齐），用户改 `agent.yaml` 即生效。
    #[serde(default = "default_llm_request_timeout_secs")]
    pub request_timeout_secs: u64,

    /// LLM HTTP 连接超时（秒），默认 30s。
    #[serde(default = "default_llm_connect_timeout_secs")]
    pub connect_timeout_secs: u64,

    /// Cache 诊断配置 (测量用)
    #[serde(default)]
    pub cache_diagnostics: CacheDiagnosticsConfig,
}

/// Cache 诊断配置 (测量用)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheDiagnosticsConfig {
    pub enabled: bool,               // env var: CYBER_JIANGHU_CACHE_DIAGNOSTICS_ENABLED
    pub system_hash_dimension: bool, // env var: CYBER_JIANGHU_CACHE_DIAGNOSTICS_SYSTEM_HASH_DIMENSION
}

impl Default for CacheDiagnosticsConfig {
    fn default() -> Self {
        Self {
            enabled: env_or("CYBER_JIANGHU_CACHE_DIAGNOSTICS_ENABLED", true),
            system_hash_dimension: env_or(
                "CYBER_JIANGHU_CACHE_DIAGNOSTICS_SYSTEM_HASH_DIMENSION",
                true,
            ),
        }
    }
}

fn default_idle_rotate_threshold() -> u32 {
    DEFAULT_IDLE_ROTATE_THRESHOLD
}

fn default_context_window_tokens() -> u32 {
    DEFAULT_CONTEXT_WINDOW_TOKENS
}

fn default_summary_trigger_ratio() -> f64 {
    DEFAULT_SUMMARY_TRIGGER_RATIO
}

fn default_keep_recent_turns() -> u32 {
    DEFAULT_KEEP_RECENT_TURNS
}

fn default_reconnect_delay_secs() -> u64 {
    DEFAULT_RECONNECT_DELAY_SECS
}

fn default_execution_result_timeout_ms() -> u64 {
    DEFAULT_EXECUTION_RESULT_TIMEOUT_MS
}

fn default_soul_cycle_report_retries() -> u32 {
    DEFAULT_SOUL_CYCLE_REPORT_RETRIES
}

fn default_soul_cycle_report_base_delay_ms() -> u64 {
    DEFAULT_SOUL_CYCLE_REPORT_BASE_DELAY_MS
}

fn default_narrative_window_size() -> usize {
    DEFAULT_NARRATIVE_WINDOW_SIZE
}

fn default_enable_streaming() -> bool {
    DEFAULT_ENABLE_STREAMING
}

fn default_llm_provider() -> String {
    DEFAULT_LLM_PROVIDER.to_string()
}

fn default_llm_temperature() -> f32 {
    DEFAULT_LLM_TEMPERATURE
}

fn default_llm_max_tokens() -> u32 {
    DEFAULT_LLM_MAX_TOKENS
}

fn default_llm_request_timeout_secs() -> u64 {
    DEFAULT_LLM_REQUEST_TIMEOUT_SECS
}

fn default_llm_connect_timeout_secs() -> u64 {
    DEFAULT_LLM_CONNECT_TIMEOUT_SECS
}

pub(crate) fn env_or<T: std::str::FromStr>(key: &str, fallback: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(fallback)
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: env_or(
                "CYBER_JIANGHU_LLM_PROVIDER",
                DEFAULT_LLM_PROVIDER.to_string(),
            ),
            base_url: None,
            api_key: None,
            model: None,
            temperature: env_or("CYBER_JIANGHU_LLM_TEMPERATURE", DEFAULT_LLM_TEMPERATURE),
            max_tokens: env_or("CYBER_JIANGHU_LLM_MAX_TOKENS", DEFAULT_LLM_MAX_TOKENS),
            fallback_models: Vec::new(),
            models: Vec::new(),
            idle_rotate_threshold: env_or(
                "CYBER_JIANGHU_IDLE_ROTATE_THRESHOLD",
                DEFAULT_IDLE_ROTATE_THRESHOLD,
            ),
            context_window_tokens: env_or(
                "CYBER_JIANGHU_CONTEXT_WINDOW_TOKENS",
                DEFAULT_CONTEXT_WINDOW_TOKENS,
            ),
            summary_trigger_ratio: env_or(
                "CYBER_JIANGHU_SUMMARY_TRIGGER_RATIO",
                DEFAULT_SUMMARY_TRIGGER_RATIO,
            ),
            keep_recent_turns: env_or("CYBER_JIANGHU_KEEP_RECENT_TURNS", DEFAULT_KEEP_RECENT_TURNS),
            reconnect_delay_secs: env_or(
                "CYBER_JIANGHU_RECONNECT_DELAY_SECS",
                DEFAULT_RECONNECT_DELAY_SECS,
            ),
            execution_result_timeout_ms: env_or(
                "CYBER_JIANGHU_EXECUTION_RESULT_TIMEOUT_MS",
                DEFAULT_EXECUTION_RESULT_TIMEOUT_MS,
            ),
            soul_cycle_report_retries: env_or(
                "CYBER_JIANGHU_SOUL_CYCLE_REPORT_RETRIES",
                DEFAULT_SOUL_CYCLE_REPORT_RETRIES,
            ),
            soul_cycle_report_base_delay_ms: env_or(
                "CYBER_JIANGHU_SOUL_CYCLE_REPORT_BASE_DELAY_MS",
                DEFAULT_SOUL_CYCLE_REPORT_BASE_DELAY_MS,
            ),
            narrative_window_size: env_or(
                "CYBER_JIANGHU_NARRATIVE_WINDOW_SIZE",
                DEFAULT_NARRATIVE_WINDOW_SIZE,
            ),
            enable_streaming: env_or("CYBER_JIANGHU_ENABLE_STREAMING", DEFAULT_ENABLE_STREAMING),
            enable_thinking: None,
            request_timeout_secs: env_or(
                "CYBER_JIANGHU_LLM_REQUEST_TIMEOUT_SECS",
                DEFAULT_LLM_REQUEST_TIMEOUT_SECS,
            ),
            connect_timeout_secs: env_or(
                "CYBER_JIANGHU_LLM_CONNECT_TIMEOUT_SECS",
                DEFAULT_LLM_CONNECT_TIMEOUT_SECS,
            ),
            cache_diagnostics: CacheDiagnosticsConfig::default(),
        }
    }
}

impl Drop for LlmConfig {
    fn drop(&mut self) {
        if let Some(ref mut key) = self.api_key {
            key.zeroize();
        }
    }
}
