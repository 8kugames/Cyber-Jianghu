// ============================================================================
// 配置管理
// ============================================================================
//
// Agent 配置结构，分为三层：
// 1. Identity - Agent 身份（持久化，不随角色变化）
// 2. Server - 服务器连接配置
// 3. Character - 当前角色（通过 Web/API 创建）
// ============================================================================

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroize;

// ============================================================================
// 导入 protocol 类型
// ============================================================================

pub use cyber_jianghu_protocol::{AvailableAction, GameRules, InitialItem, WorldTime};

// ============================================================================
// 每服务器设备身份配置（device.yaml）
// ============================================================================

/// 每服务器设备身份（device.yaml）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub device_id: Uuid,
    pub auth_token: String,
    pub server_url: String,
}

impl DeviceConfig {
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let yaml = serde_yaml::to_string(self).context("Failed to serialize DeviceConfig")?;
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &yaml)?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let yaml = fs::read_to_string(path).context("Failed to read device.yaml")?;
        serde_yaml::from_str(&yaml).context("Failed to parse device.yaml")
    }

    pub fn ws_url_with_token(&self, ws_url: &str, agent_id: Option<Uuid>) -> String {
        let mut url = format!(
            "{}?device_id={}&token={}",
            ws_url, self.device_id, self.auth_token
        );
        if let Some(id) = agent_id {
            url.push_str(&format!("&agent_id={}", id));
        }
        url
    }
}

/// 计算服务器目录 key（从 WebSocket URL 派生）
pub fn server_key(ws_url: &str) -> String {
    let url = Url::parse(ws_url).unwrap_or_else(|_| {
        Url::parse(&format!("ws://{}", ws_url)).expect("ws:// prefix always produces valid URL")
    });
    let host = url.host_str().unwrap_or("localhost");
    let port = url.port().map(|p| format!("-{}", p)).unwrap_or_default();
    format!("{}{}", host.replace(['.', ':', '[', ']'], "-"), port)
}

/// Convert WebSocket URL to HTTP URL.
/// e.g. `ws://localhost:23333/ws` -> `http://localhost:23333`
pub fn ws_to_http_url(ws_url: &str) -> String {
    ws_url
        .replace("ws://", "http://")
        .replace("wss://", "https://")
        .rsplit_once('/')
        .map(|(base, _)| base.to_string())
        .unwrap_or_else(|| ws_url.to_string())
}

// ============================================================================
// 服务器配置
// ============================================================================

/// 服务器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// WebSocket URL（用于实时通信）
    #[serde(default = "default_ws_url")]
    pub ws_url: String,

    /// HTTP URL（用于 API 调用）
    #[serde(default = "default_http_url")]
    pub http_url: String,
}

fn default_ws_url() -> String {
    "ws://localhost:23333/ws".to_string()
}

fn default_http_url() -> String {
    "http://localhost:23333".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            ws_url: default_ws_url(),
            http_url: default_http_url(),
        }
    }
}

impl ServerConfig {
    /// 生成带认证参数的 WebSocket URL
    pub fn ws_url_with_token(
        &self,
        device_id: Uuid,
        auth_token: &str,
        agent_id: Option<Uuid>,
    ) -> String {
        let mut url = format!(
            "{}?device_id={}&token={}",
            self.ws_url, device_id, auth_token
        );
        if let Some(id) = agent_id {
            url.push_str(&format!("&agent_id={}", id));
        }
        url
    }
}

// ============================================================================
// 角色配置（侠客）
// ============================================================================

/// 语言风格配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LanguageStyleConfig {
    /// 语调：豪迈/温和/冷漠/狡黠
    #[serde(default)]
    pub tone: Option<String>,

    /// 说话特点
    #[serde(default)]
    pub speech_patterns: Vec<String>,
}

/// 角色目标配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GoalsConfig {
    /// 短期目标
    #[serde(default)]
    pub short_term: Option<String>,

    /// 长远目标
    #[serde(default)]
    pub long_term: Option<String>,
}

/// 角色状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CharacterStatus {
    /// 存活
    #[default]
    Alive,
    /// 死亡
    Dead,
    /// 归隐（转生）
    Retired,
}

/// 角色配置（侠客）
///
/// 通过 Web 面板或 HTTP API 创建。
/// 角色死亡后可以转世，此时 agent_id 会变化。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CharacterConfig {
    /// 服务器分配的角色 ID（注册后由服务器返回）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<Uuid>,

    // === 基本信息 ===
    /// 姓名
    pub name: String,

    /// 年龄
    #[serde(default = "default_age")]
    pub age: u8,

    /// 性别
    #[serde(default = "default_gender")]
    pub gender: String,

    /// 外貌描述
    #[serde(default)]
    pub appearance: Option<String>,

    /// 身份背景（如：江湖游侠、商人、书生）
    #[serde(default)]
    pub identity: Option<String>,

    // === 性格特征 ===
    #[serde(default)]
    pub personality: Vec<String>,

    // === 核心价值观 ===
    #[serde(default)]
    pub values: Vec<String>,

    // === 语言风格 ===
    #[serde(default)]
    pub language_style: LanguageStyleConfig,

    // === 当前目标 ===
    #[serde(default)]
    pub goals: GoalsConfig,

    // === 系统提示词（自动生成或自定义） ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    // === 注册时服务器返回的信息 ===
    /// 注册时间（注册成功时记录）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<chrono::DateTime<chrono::Utc>>,

    /// 先天属性（注册时从服务器获取，用于对比成长）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birth_attributes: Option<std::collections::HashMap<String, i32>>,

    // === 服务器关联 ===
    /// 所属服务器的 HTTP URL（用于区分不同服务器的角色）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,

    /// 最近一次连接时的现实时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connected_real_time: Option<chrono::DateTime<chrono::Utc>>,

    /// 最近一次连接时的游戏时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connected_world_time: Option<cyber_jianghu_protocol::WorldTime>,

    /// 角色状态
    #[serde(default)]
    pub status: CharacterStatus,

    /// 纪传体传记（死亡/归隐时由 LLM 生成，汇总经历日志）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biography: Option<String>,
}

fn default_age() -> u8 {
    25
}

fn default_gender() -> String {
    "男".to_string()
}

impl CharacterConfig {
    /// 生成系统提示词
    ///
    /// 如果用户没有提供自定义 system_prompt，则根据角色信息自动生成
    pub fn generate_system_prompt(&self) -> String {
        if let Some(ref prompt) = self.system_prompt {
            return prompt.clone();
        }

        let mut parts = vec![];

        // 基本信息
        parts.push(format!(
            "你是{}，一位{}岁的{}。",
            self.name, self.age, self.gender
        ));

        // 外貌
        if let Some(ref appearance) = self.appearance {
            parts.push(format!("外貌：{}。", appearance));
        }

        // 身份
        if let Some(ref identity) = self.identity {
            parts.push(format!("身份：{}。", identity));
        }

        // 性格
        if !self.personality.is_empty() {
            parts.push(format!("性格：{}。", self.personality.join("、")));
        }

        // 价值观
        if !self.values.is_empty() {
            parts.push(format!("核心价值观：{}。", self.values.join("；")));
        }

        // 语言风格
        if let Some(ref tone) = self.language_style.tone {
            parts.push(format!("说话风格{}。", tone));
        }
        if !self.language_style.speech_patterns.is_empty() {
            parts.push(format!(
                "语言特点：{}。",
                self.language_style.speech_patterns.join("，")
            ));
        }

        // 目标
        if let Some(ref short_term) = self.goals.short_term {
            parts.push(format!("当前目标：{}。", short_term));
        }
        if let Some(ref long_term) = self.goals.long_term {
            parts.push(format!("长远目标：{}。", long_term));
        }

        parts.join("\n")
    }

    /// 检查角色是否已注册
    pub fn is_registered(&self) -> bool {
        self.agent_id.is_some()
    }

    /// 从文件加载角色配置
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read character config from {:?}", path.as_ref()))?;
        serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse character config from {:?}", path.as_ref()))
    }

    /// 保存角色配置到文件（原子写入：先写临时文件再 rename）
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content =
            serde_yaml::to_string(self).context("Failed to serialize character config")?;
        let path = path.as_ref();
        let tmp_path = path.with_extension("tmp");
        std::fs::write(&tmp_path, &content)
            .with_context(|| format!("Failed to write character config to {:?}", tmp_path))?;
        std::fs::rename(&tmp_path, path)
            .with_context(|| format!("Failed to rename character config at {:?}", path))?;
        Ok(())
    }
}

// ============================================================================
// 运行时配置
// ============================================================================

/// 运行模式
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    /// Cognitive 模式（默认）- 内置 LLM 决策，无需外部调度器
    #[default]
    Cognitive,
    /// Claw 模式 - 为 OpenClaw 等外部助手提供 WebSocket + HTTP API
    /// LLM 由外部 OpenClaw 提供，Agent 内部认知引擎通过 OpenClawBridge 调用
    Claw,
}

impl std::fmt::Display for RuntimeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeMode::Claw => write!(f, "claw"),
            RuntimeMode::Cognitive => write!(f, "cognitive"),
        }
    }
}

fn default_token_opt_enabled() -> bool {
    true
}

fn default_true() -> bool {
    true
}

/// 运行时配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// 运行模式
    #[serde(default)]
    pub mode: RuntimeMode,

    /// HTTP API 端口
    /// 0 = 在 23340~23999 范围内随机选择
    #[serde(default)]
    pub port: u16,

    /// 停止 LLM 调用
    #[serde(default)]
    pub llm_disabled: bool,

    /// 自动重生开关：角色死亡后自动转世重生（复用角色信息）
    #[serde(default = "default_true")]
    pub auto_rebirth: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::Cognitive,
            port: 0,
            llm_disabled: false,
            auto_rebirth: true,
        }
    }
}

// ============================================================================
// ============================================================================
// LLM 配置（仅 Cognitive 模式使用）
// ============================================================================

const DEFAULT_LLM_PROVIDER: &str = "ollama";
const DEFAULT_LLM_TEMPERATURE: f32 = 0.7;
const DEFAULT_LLM_MAX_TOKENS: u32 = 8192;

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
const DEFAULT_IDLE_ROTATE_THRESHOLD: u32 = 24;
const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 32000;
const DEFAULT_SUMMARY_TRIGGER_RATIO: f64 = 0.75;
const DEFAULT_KEEP_RECENT_TURNS: u32 = 4;
const DEFAULT_RECONNECT_DELAY_SECS: u64 = 5;
const DEFAULT_EXECUTION_RESULT_TIMEOUT_MS: u64 = 3000;
const DEFAULT_SOUL_CYCLE_REPORT_RETRIES: u32 = 3;
const DEFAULT_SOUL_CYCLE_REPORT_BASE_DELAY_MS: u64 = 100;
pub(crate) const DEFAULT_NARRATIVE_WINDOW_SIZE: usize = 5;
/// 语义去重历史窗口：比较最近 N 条同类 intent 的语义相似度
pub(crate) const DEFAULT_SEMANTIC_DEDUP_HISTORY: usize = 1;
const DEFAULT_ENABLE_STREAMING: bool = true;

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

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: DEFAULT_LLM_PROVIDER.to_string(),
            base_url: None,
            api_key: None,
            model: None,
            temperature: DEFAULT_LLM_TEMPERATURE,
            max_tokens: DEFAULT_LLM_MAX_TOKENS,
            fallback_models: Vec::new(),
            models: Vec::new(),
            idle_rotate_threshold: DEFAULT_IDLE_ROTATE_THRESHOLD,
            context_window_tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
            summary_trigger_ratio: DEFAULT_SUMMARY_TRIGGER_RATIO,
            keep_recent_turns: DEFAULT_KEEP_RECENT_TURNS,
            reconnect_delay_secs: DEFAULT_RECONNECT_DELAY_SECS,
            execution_result_timeout_ms: DEFAULT_EXECUTION_RESULT_TIMEOUT_MS,
            soul_cycle_report_retries: DEFAULT_SOUL_CYCLE_REPORT_RETRIES,
            soul_cycle_report_base_delay_ms: DEFAULT_SOUL_CYCLE_REPORT_BASE_DELAY_MS,
            narrative_window_size: DEFAULT_NARRATIVE_WINDOW_SIZE,
            enable_streaming: DEFAULT_ENABLE_STREAMING,
            enable_thinking: None,
        }
    }
}

impl LlmConfig {
    pub fn from_env() -> Self {
        Self {
            provider: std::env::var("CYBER_JIANGHU_LLM_PROVIDER")
                .unwrap_or_else(|_| DEFAULT_LLM_PROVIDER.to_string()),
            base_url: std::env::var("CYBER_JIANGHU_LLM_BASE_URL").ok(),
            api_key: std::env::var("CYBER_JIANGHU_LLM_API_KEY").ok(),
            model: std::env::var("CYBER_JIANGHU_LLM_MODEL").ok(),
            temperature: std::env::var("CYBER_JIANGHU_LLM_TEMPERATURE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_LLM_TEMPERATURE),
            max_tokens: std::env::var("CYBER_JIANGHU_LLM_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_LLM_MAX_TOKENS),
            fallback_models: std::env::var("CYBER_JIANGHU_LLM_FALLBACK_MODELS")
                .ok()
                .map(|s| {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            models: Vec::new(),
            idle_rotate_threshold: DEFAULT_IDLE_ROTATE_THRESHOLD,
            context_window_tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
            summary_trigger_ratio: DEFAULT_SUMMARY_TRIGGER_RATIO,
            keep_recent_turns: DEFAULT_KEEP_RECENT_TURNS,
            reconnect_delay_secs: DEFAULT_RECONNECT_DELAY_SECS,
            execution_result_timeout_ms: DEFAULT_EXECUTION_RESULT_TIMEOUT_MS,
            soul_cycle_report_retries: DEFAULT_SOUL_CYCLE_REPORT_RETRIES,
            soul_cycle_report_base_delay_ms: DEFAULT_SOUL_CYCLE_REPORT_BASE_DELAY_MS,
            narrative_window_size: DEFAULT_NARRATIVE_WINDOW_SIZE,
            enable_streaming: DEFAULT_ENABLE_STREAMING,
            enable_thinking: None,
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

// ============================================================================
// 记忆系统配置
// ============================================================================

/// 记忆系统配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// 是否启用记忆系统
    #[serde(default = "default_memory_enabled")]
    pub enabled: bool,

    /// 工作记忆容量（保留最近 N 条事件）
    #[serde(default = "default_working_memory_size")]
    pub working_memory_size: usize,

    /// 情景记忆保存阈值（重要性 >= 此值的事件会被保存）
    #[serde(default = "default_episodic_threshold")]
    pub episodic_threshold: f32,

    /// 遗忘机制运行间隔（tick 数）
    /// 基于 tick_duration=60s 时，84 ticks ≈ 84 分钟
    #[serde(default = "default_forgetting_interval_ticks")]
    pub forgetting_interval_ticks: i64,
}

fn default_memory_enabled() -> bool {
    true
}

fn default_working_memory_size() -> usize {
    20
}

fn default_episodic_threshold() -> f32 {
    0.3
}

fn default_forgetting_interval_ticks() -> i64 {
    84
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            working_memory_size: 20,
            episodic_threshold: 0.3,
            forgetting_interval_ticks: 84,
        }
    }
}

// ============================================================================
// Token 优化配置
// ============================================================================

/// Token 优化总开关与子模块配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenOptimizationConfig {
    /// 总开关（默认开启）
    #[serde(default = "default_token_opt_enabled")]
    pub enabled: bool,
    /// ReflectorSoul 优化：消灭重试循环
    pub reflector: ReflectorOptConfig,
    /// Attention Controller
    pub attention: AttentionConfig,
    /// Delta Engine
    pub delta: DeltaConfig,
}

impl Default for TokenOptimizationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reflector: ReflectorOptConfig::default(),
            attention: AttentionConfig::default(),
            delta: DeltaConfig::default(),
        }
    }
}

/// ReflectorSoul 优化配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReflectorOptConfig {
    /// 启用 self-correction：被驳回后调用 LLM 纠正一次
    pub self_correction: bool,
    /// 双重拒绝后直接 chaos_fallback（不再重试）
    pub chaos_on_double_reject: bool,
    /// self-correction LLM 失败累计达到此值后，跳过 self_correct 直接 chaos_fallback
    pub chaos_on_llm_fail: u32,
}

/// Attention Controller 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AttentionConfig {
    pub max_focus_items: usize,
    pub first_tick_focus_cap: usize,
    pub critical_auto_include: bool,
}

/// Delta Engine 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeltaConfig {
    pub change_percentage_threshold: f32,
    /// 生存驱动 Critical 阈值（>= 此值标 Critical，< 此值标 Important）
    pub survival_critical_urgency_threshold: u8,
}

impl Default for ReflectorOptConfig {
    fn default() -> Self {
        Self {
            self_correction: true,
            chaos_on_double_reject: true,
            chaos_on_llm_fail: 2,
        }
    }
}

impl Default for AttentionConfig {
    fn default() -> Self {
        Self {
            max_focus_items: 5,
            first_tick_focus_cap: 15,
            critical_auto_include: true,
        }
    }
}

impl Default for DeltaConfig {
    fn default() -> Self {
        Self {
            change_percentage_threshold: 0.1,
            survival_critical_urgency_threshold: 5,
        }
    }
}

/// 角色生成约束 -- schema 驱动的单一数据源。
///
/// YAML 定义字段规格（path + type + constraints），prompt 和 validate 均从同一 schema 动态生成。
/// 添加/修改约束只需改 YAML，零代码改动。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterGenerationConfig {
    pub world_setting: String,
    pub fields: Vec<FieldSpec>,
}

/// 字段约束类型 -- 4 种覆盖当前所有字段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldConstraints {
    String {
        required: bool,
        #[serde(default)]
        min_chars: usize,
        max_chars: usize,
        #[serde(default)]
        prompt_text: Option<String>,
    },
    Integer {
        required: bool,
        min: u32,
        max: u32,
    },
    Enum {
        required: bool,
        options: Vec<String>,
        #[serde(default)]
        prompt_text: Option<String>,
    },
    EnumArray {
        required: bool,
        options: Vec<String>,
        min_count: usize,
        max_count: usize,
        #[serde(default)]
        extra_prompt: Option<String>,
    },
}

/// 字段规格 -- path 支持 dot notation（如 language_style.tone）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSpec {
    pub path: String,
    #[serde(flatten)]
    pub constraints: FieldConstraints,
}

// ============================================================================
// 完整配置
// ============================================================================

/// 完整配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 服务器配置
    #[serde(default)]
    pub server: ServerConfig,

    /// 运行时配置
    #[serde(default)]
    pub runtime: RuntimeConfig,

    /// LLM 配置（Cognitive 模式直连 LLM，Claw 模式通过 OpenClawBridge）
    #[serde(default)]
    pub llm: LlmConfig,

    /// ReflectorSoul LLM 配置（可选，未配置时继承 llm）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_reflector: Option<LlmConfig>,

    /// 记忆系统配置
    #[serde(default)]
    pub memory: MemoryConfig,

    /// 游戏规则（从服务器获取）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_rules: Option<GameRules>,

    /// 配置文件路径（运行时设置，不序列化）
    #[serde(skip)]
    pub config_path: PathBuf,

    /// 服务器数据目录
    /// 默认 ~/.cyber-jianghu/servers/
    #[serde(default)]
    pub servers_dir: PathBuf,

    /// 地魂（EarthSoul）配置 — tool result 预算 & 循环检测
    #[serde(default)]
    pub earth_soul: crate::soul::earth::config::EarthSoulConfig,

    /// Token 优化配置（总开关默认关闭）
    #[serde(default)]
    pub token_optimization: TokenOptimizationConfig,

    /// 角色生成约束（必填，缺失时 serde 报错 fail-fast）
    pub character_generation: CharacterGenerationConfig,
}

impl Config {
    /// 从文件加载配置
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_display = path.as_ref().display().to_string();
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path_display))?;

        let config: Config =
            serde_yaml::from_str(&content).with_context(|| "Failed to parse config file")?;

        Ok(config)
    }

    /// 保存配置到文件（原子写入：先写临时文件，再 rename 替换）
    ///
    /// 避免进程中断时文件被截断为空。
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();
        let path_display = path.display().to_string();
        let yaml =
            serde_yaml::to_string(self).with_context(|| "Failed to serialize config to YAML")?;

        // 确保目录存在
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        // 原子写入：先写临时文件，再 rename
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &yaml)
            .with_context(|| format!("Failed to write temp config file: {}", tmp_path.display()))?;

        if let Err(e) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            anyhow::bail!("Failed to replace config file {}: {}", path_display, e);
        }

        Ok(())
    }

    /// 获取重生延迟 tick 数（0 = 不自动重生）
    pub fn rebirth_delay_ticks(&self) -> i32 {
        self.game_rules
            .as_ref()
            .map(|r| r.rebirth_delay_ticks)
            .unwrap_or(0)
    }

    /// 更新游戏规则
    pub fn update_game_rules(&mut self, game_rules: GameRules) {
        // 保存 available_actions 到本地文件
        // 优先 CYBER_JIANGHU_CONFIG_DIR，回退 CYBER_JIANGHU_DATA_DIR/config，再回退默认路径
        let config_dir = std::env::var("CYBER_JIANGHU_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("CYBER_JIANGHU_DATA_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| {
                        dirs::home_dir()
                            .unwrap_or_else(|| PathBuf::from("."))
                            .join(".cyber-jianghu")
                    })
                    .join("config")
            });
        let actions_path = config_dir.join("actions.json");

        // 确保目录存在
        if let Err(e) = fs::create_dir_all(&config_dir) {
            tracing::warn!("创建配置目录失败: {}", e);
        } else {
            // 序列化并保存
            match serde_json::to_string_pretty(&game_rules.available_actions) {
                Ok(json) => {
                    if let Err(e) = fs::write(&actions_path, json) {
                        tracing::warn!("保存 actions.json 失败: {}", e);
                    } else {
                        tracing::debug!(
                            "已保存 {} 个动作到 {:?}",
                            game_rules.available_actions.len(),
                            actions_path
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("序列化 actions 失败: {}", e);
                }
            }
        }

        self.game_rules = Some(game_rules);
    }

    /// 获取 ReflectorSoul LLM 配置（带回退逻辑）
    pub fn get_reflector_llm_config(&self) -> &LlmConfig {
        self.llm_reflector.as_ref().unwrap_or(&self.llm)
    }

    /// 获取指定服务器的数据目录
    pub fn server_dir(&self, ws_url: &str) -> PathBuf {
        self.servers_dir.join(server_key(ws_url))
    }

    /// 获取指定服务器的 device.yaml 路径
    pub fn device_yaml_path(&self, ws_url: &str) -> PathBuf {
        self.server_dir(ws_url).join("device.yaml")
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 CharacterGenerationConfig (minimal schema)
    fn test_cg() -> CharacterGenerationConfig {
        CharacterGenerationConfig {
            world_setting: "武侠架空世界".into(),
            fields: vec![FieldSpec {
                path: "age".into(),
                constraints: FieldConstraints::Integer {
                    required: true,
                    min: 16,
                    max: 60,
                },
            }],
        }
    }

    #[test]
    fn test_character_config_generate_system_prompt() {
        let character = CharacterConfig {
            name: "李逍遥".to_string(),
            age: 25,
            gender: "男".to_string(),
            appearance: Some("身材修长，剑眉星目".to_string()),
            identity: Some("江湖游侠".to_string()),
            personality: vec!["豪爽".to_string(), "重情重义".to_string()],
            values: vec!["侠之大者，为国为民".to_string()],
            language_style: LanguageStyleConfig {
                tone: Some("豪迈".to_string()),
                speech_patterns: vec!["喜欢用江湖切口".to_string()],
            },
            goals: GoalsConfig {
                short_term: Some("寻找失散的师妹".to_string()),
                long_term: Some("成为一代大侠".to_string()),
            },
            ..Default::default()
        };

        let prompt = character.generate_system_prompt();
        assert!(prompt.contains("李逍遥"));
        assert!(prompt.contains("25岁"));
        assert!(prompt.contains("豪爽"));
        assert!(prompt.contains("寻找失散的师妹"));
    }

    #[test]
    fn test_reflector_llm_inheritance() {
        let mut llm = LlmConfig::default();
        llm.provider = "ollama".to_string();
        llm.model = Some("qwen2.5:14b".to_string());

        let config = Config {
            server: ServerConfig::default(),
            runtime: RuntimeConfig::default(),
            llm,
            llm_reflector: None,
            memory: MemoryConfig::default(),
            game_rules: None,
            config_path: PathBuf::from("/test/config.yaml"),
            servers_dir: PathBuf::new(),
            earth_soul: crate::soul::earth::config::EarthSoulConfig::default(),
            token_optimization: TokenOptimizationConfig::default(),
            character_generation: test_cg(),
        };
        assert_eq!(
            config.get_reflector_llm_config().model,
            Some("qwen2.5:14b".to_string())
        );
    }

    #[test]
    fn test_reflector_llm_override() {
        let mut llm = LlmConfig::default();
        llm.provider = "ollama".to_string();
        llm.model = Some("qwen2.5:14b".to_string());

        let mut llm_reflector = LlmConfig::default();
        llm_reflector.provider = "ollama".to_string();
        llm_reflector.model = Some("qwen2.5:32b".to_string());

        let config = Config {
            server: ServerConfig::default(),
            runtime: RuntimeConfig::default(),
            llm,
            llm_reflector: Some(llm_reflector),
            memory: MemoryConfig::default(),
            game_rules: None,
            config_path: PathBuf::from("/test/config.yaml"),
            servers_dir: PathBuf::new(),
            earth_soul: crate::soul::earth::config::EarthSoulConfig::default(),
            token_optimization: TokenOptimizationConfig::default(),
            character_generation: test_cg(),
        };
        assert_eq!(
            config.get_reflector_llm_config().model,
            Some("qwen2.5:32b".to_string())
        );
    }
}
