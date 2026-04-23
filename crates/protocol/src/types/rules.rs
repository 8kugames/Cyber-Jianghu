//! 游戏规则相关类型
//!
//! 包含游戏规则和世界观规则
//!
//! # 设计哲学
//!
//! 本协议 crate 为 Cyber-Jianghu 武侠 MMO 设计。
//!
//! ## 默认值说明
//!
//! 本类型中的 `Default` 实现和 `fn default_*()` 辅助函数包含武侠主题的默认值，
//! 这些默认值用于：
//! - 开发/测试环境（无配置文件时）
//! - 配置缺失时的安全降级
//!
//! **生产环境应通过 `game_rules.yaml` 配置所有值**，而非依赖代码默认值。
//!
//! ## 武侠主题默认值示例
//!
//! - 呼唤词："喂", "哎", "侠客", "朋友"
//! - 限制区域："admin", "vault", "secret"
//! - 高价值物品："silver", "gold"
//! - 默认回应："何事？"
//!
//! 这些默认值确保系统在无配置时仍可运行，但**不代表通用的设计决策**。
//! 不同的游戏主题应通过配置文件覆盖这些值。

use serde::{Deserialize, Serialize};

fn default_survival_threshold() -> i32 {
    30
}

use super::entities::{AvailableAction, InitialItem};

/// 游戏规则
///
/// 服务端下发的游戏规则配置，包含可用动作和初始物品
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameRules {
    /// Tick 周期（秒）
    pub tick_duration_secs: u64,

    /// 可用动作列表
    pub available_actions: Vec<AvailableAction>,

    /// 初始物品（注册时发放）
    pub initial_items: Vec<InitialItem>,

    /// 生存相关动作列表（hunger/thirst 低于阈值时绕过 ReflectorSoul 审查）
    #[serde(default)]
    pub survival_actions: Vec<String>,

    /// 生存底线阈值（hunger/thirst 低于此值时触发 survival 动作绕过审查）
    #[serde(default = "default_survival_threshold")]
    pub survival_threshold: i32,

    /// 生存攻击阈值（hunger/thirst 低于此值时注入攻击/交易提示）
    #[serde(default = "default_critical_attack_threshold")]
    pub critical_attack_threshold: i32,

    /// 规则版本（用于检测变更）
    pub version: String,

    /// 最后更新时间
    pub last_updated: String,

    /// Intent 批次配置（multi-Intent）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent_batch: Option<IntentBatchConfig>,

    /// 天魂（ReflectorSoul）叙事生成配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflector_narrative: Option<ReflectorNarrativeConfig>,

    /// 即时事件处理配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immediate_events: Option<ImmediateEventConfig>,

    /// 死亡后自动重生延迟 tick 数（0 = 不自动重生）
    #[serde(default)]
    pub rebirth_delay_ticks: i32,
}

fn default_critical_attack_threshold() -> i32 {
    15
}

// ============================================================================
// Multi-Intent 配置
// ============================================================================

/// Intent 批次配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentBatchConfig {
    /// 每 tick 最大 Intent 数
    #[serde(default = "default_max_intents")]
    pub max_intents_per_tick: usize,

    /// 三魂循环最大重试次数
    #[serde(default = "default_max_retries")]
    pub max_retries: i32,

    /// 是否启用 Pipeline 执行
    #[serde(default = "default_true")]
    pub pipeline_execution_enabled: bool,

    /// 是否允许部分执行
    #[serde(default = "default_true")]
    pub partial_execution_enabled: bool,

    /// 分级审核配置
    #[serde(default)]
    pub llm_validation: GradedValidationConfig,

    /// LLM 连续失败多少 tick 后激活 chaos 模式
    #[serde(default = "default_llm_chaos_threshold")]
    pub llm_chaos_threshold: u32,
}

fn default_llm_chaos_threshold() -> u32 {
    12
}

fn default_max_intents() -> usize {
    5
}
fn default_max_retries() -> i32 {
    3
}

/// 分级审核配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradedValidationConfig {
    /// 强制 LLM 审核的 action_type（speak/shout/whisper 等）
    #[serde(default = "default_always_types")]
    pub always_types: Vec<String>,

    /// 动态审核的 action_type（根据 action_data 判断）
    #[serde(default = "default_adaptive_types")]
    pub adaptive_types: Vec<String>,

    /// 跳过 LLM 审核的 action_type
    #[serde(default = "default_skip_types")]
    pub skip_types: Vec<String>,

    /// 每 tick 至少审核的 Intent 数量
    #[serde(default = "default_minimum_per_tick")]
    pub minimum_per_tick: usize,

    /// 限制区域 node_id 前缀/关键词（move 审核用）
    #[serde(default = "default_restricted_area_keywords")]
    pub restricted_area_keywords: Vec<String>,

    /// 高价值物品 item_id 前缀/关键词（trade/steal/give 审核用）
    #[serde(default = "default_high_value_item_keywords")]
    pub high_value_item_keywords: Vec<String>,

    /// Adaptive 审核字段映射（数据驱动）
    ///
    /// 格式: { "action_type": "action_data_field_name" }
    /// 例如: { "移动": "target_location", "偷窃": "item_id", "给予": "item_id" }
    /// 当 action_type 在此映射中时，检查对应字段的值是否匹配对应的关键词列表：
    /// - "target_location" → 检查 restricted_area_keywords
    /// - "item_id" → 检查 high_value_item_keywords
    /// - 其他字段 → 默认需要 LLM 审核
    #[serde(default = "default_adaptive_field_mapping")]
    pub adaptive_field_mapping: std::collections::HashMap<String, String>,
}

fn default_always_types() -> Vec<String> {
    // 武侠主题默认值：说话类动作强制审核
    vec!["说话".into(), "大喊".into(), "私语".into()]
}

fn default_adaptive_types() -> Vec<String> {
    // 武侠主题默认值：交易/移动类动作动态审核
    vec!["偷窃".into(), "给予".into(), "移动".into()]
}

fn default_skip_types() -> Vec<String> {
    // 武侠主题默认值：空闲动作跳过审核
    vec!["休息".into(), "wait".into()]
}

fn default_restricted_area_keywords() -> Vec<String> {
    // 武侠主题默认值：限制区域地点前缀
    // 生产环境应通过 game_rules.yaml 配置
    vec!["admin".into(), "vault".into(), "secret".into()]
}

fn default_high_value_item_keywords() -> Vec<String> {
    // 武侠主题默认值：高价值物品前缀
    // 生产环境应通过 game_rules.yaml 配置
    vec!["银子".into(), "silver".into(), "gold".into()]
}

fn default_adaptive_field_mapping() -> std::collections::HashMap<String, String> {
    [
        ("移动".into(), "target_location".into()),
        ("偷窃".into(), "item_id".into()),
        ("给予".into(), "item_id".into()),
    ]
    .into()
}

/// 天魂（ReflectorSoul）叙事生成配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectorNarrativeConfig {
    /// 是否启用 LLM 生成（false 时使用空 NarrativeContext）
    #[serde(default = "default_true")]
    pub enable_llm_generation: bool,

    /// 是否启用语义缓存
    #[serde(default = "default_true")]
    pub cache_enabled: bool,

    /// 缓存大小
    #[serde(default = "default_cache_size")]
    pub cache_size: usize,

    /// 缓存 TTL（秒）
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_seconds: i64,

    /// 是否使用 few-shot 示例
    #[serde(default = "default_true")]
    pub few_shot_examples: bool,

    /// 数值泄露检测配置
    #[serde(default)]
    pub leak_detection: LeakDetectionConfig,
}

/// 数值泄露检测配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeakDetectionConfig {
    /// 是否启用泄露检测
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// 高风险阈值（>=此分数触发重试）
    #[serde(default = "default_suspicion_threshold")]
    pub suspicion_threshold: u8,

    /// 最大重试次数
    #[serde(default = "default_max_retry")]
    pub max_retry: usize,
}

// Default helpers

fn default_true() -> bool {
    true
}

fn default_cache_size() -> usize {
    1000
}

fn default_cache_ttl() -> i64 {
    300
}

fn default_suspicion_threshold() -> u8 {
    65
}

fn default_max_retry() -> usize {
    2
}

fn default_minimum_per_tick() -> usize {
    1
}

impl Default for GradedValidationConfig {
    fn default() -> Self {
        Self {
            always_types: default_always_types(),
            adaptive_types: default_adaptive_types(),
            skip_types: default_skip_types(),
            minimum_per_tick: default_minimum_per_tick(),
            restricted_area_keywords: default_restricted_area_keywords(),
            high_value_item_keywords: default_high_value_item_keywords(),
            adaptive_field_mapping: default_adaptive_field_mapping(),
        }
    }
}

impl Default for LeakDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            suspicion_threshold: default_suspicion_threshold(),
            max_retry: default_max_retry(),
        }
    }
}

impl Default for ReflectorNarrativeConfig {
    fn default() -> Self {
        Self {
            enable_llm_generation: true,
            cache_enabled: true,
            cache_size: 1000,
            cache_ttl_seconds: 300,
            few_shot_examples: true,
            leak_detection: LeakDetectionConfig::default(),
        }
    }
}

// ============================================================================
// 即时事件配置
// ============================================================================

/// 即时事件处理配置
///
/// 控制 Agent 如何响应 Server 下发的即时事件（speak/whisper 等）
/// 以及 Agent 发送意图的路由策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImmediateEventConfig {
    /// 即时路由动作列表（这些动作走即时通道，不占 tick 配额）
    ///
    /// Agent 发送这些类型的意图时，会通过 `immediate_msg_tx` 立即发送给服务器，
    /// 而不等待主 tick 周期。适用于说话类动作（speak, whisper）。
    #[serde(default = "default_immediate_routing_actions")]
    pub immediate_routing_actions: Vec<String>,

    /// 即时决策规则（数据驱动，替代硬编码 call_keywords/default_response）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_rules: Option<ImmediateDecisionRules>,
}

/// 即时决策规则（数据驱动）
///
/// 控制即时事件的 TTL、LLM 调用超时、队列容量等参数。
/// 所有参数均可通过 game_rules.yaml 外部配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImmediateDecisionRules {
    /// 当前执行这些动作时，延迟响应
    #[serde(default)]
    pub conflict_actions: Vec<String>,
    /// 事件 TTL（ms）：超过此时间的事件视为过期
    #[serde(default = "default_event_ttl_ms")]
    pub event_ttl_ms: u64,
    /// LLM 认知调用超时（ms），应 < event_ttl_ms
    #[serde(default = "default_cognitive_timeout_ms")]
    pub cognitive_timeout_ms: u64,
    /// 最大待处理事件队列容量
    #[serde(default = "default_max_pending_events")]
    pub max_pending_events: usize,
    /// LLM 调用前的最大事件内容长度（截断长事件）
    #[serde(default = "default_max_event_context_chars")]
    pub max_event_context_chars: usize,
    /// 每 tick 最大即时 LLM 调用次数（防止 O(n²) 扇出）
    /// 超限的事件直接 DeferToMainTick
    #[serde(default = "default_max_llm_calls_per_tick")]
    pub max_llm_calls_per_tick: usize,
    /// 每 tick 最大即时意图发送次数（含 RespondNow + 天魂 speech routing）
    /// 超限的即时意图降级为 DeferToMainTick
    #[serde(default = "default_max_immediate_intents_per_tick")]
    pub max_immediate_intents_per_tick: usize,
    /// 即时意图优先级（高于普通 tick 意图）
    #[serde(default = "default_immediate_intent_priority")]
    pub immediate_intent_priority: i32,
}

fn default_event_ttl_ms() -> u64 {
    5000
}
fn default_cognitive_timeout_ms() -> u64 {
    4000
}
fn default_max_pending_events() -> usize {
    32
}
fn default_max_event_context_chars() -> usize {
    200
}
fn default_max_llm_calls_per_tick() -> usize {
    9
}
fn default_max_immediate_intents_per_tick() -> usize {
    3
}
fn default_immediate_intent_priority() -> i32 {
    10
}

fn default_immediate_routing_actions() -> Vec<String> {
    // 武侠主题默认值：说话类动作走即时通道
    vec!["说话".into(), "私语".into()]
}

impl Default for ImmediateDecisionRules {
    fn default() -> Self {
        Self {
            conflict_actions: vec![],
            event_ttl_ms: default_event_ttl_ms(),
            cognitive_timeout_ms: default_cognitive_timeout_ms(),
            max_pending_events: default_max_pending_events(),
            max_event_context_chars: default_max_event_context_chars(),
            max_llm_calls_per_tick: default_max_llm_calls_per_tick(),
            max_immediate_intents_per_tick: default_max_immediate_intents_per_tick(),
            immediate_intent_priority: default_immediate_intent_priority(),
        }
    }
}

impl Default for ImmediateEventConfig {
    fn default() -> Self {
        Self {
            immediate_routing_actions: default_immediate_routing_actions(),
            decision_rules: None,
        }
    }
}

// ============================================================================
// 世界观规则
// ============================================================================

/// 时代设定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EraSettings {
    /// 时代名称
    pub name: String,

    /// 技术水平上限
    pub tech_level: String,

    /// 社会形态
    pub social_structure: String,
}

/// 世界观规则（服务端下发 + SDK 内置基础）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldBuildingRules {
    /// 规则版本
    pub version: String,

    /// 时代设定
    pub era: EraSettings,

    /// 允许的概念（内力、轻功等）
    pub allowed_concepts: Vec<String>,

    /// 禁止的概念（魔法、现代科技等）
    pub forbidden_concepts: Vec<String>,

    /// 叙事规则（自然语言，供 LLM 理解）
    pub narrative_rules: String,

    /// 最后更新时间
    pub last_updated: String,
}

impl Default for WorldBuildingRules {
    fn default() -> Self {
        Self {
            version: "0.0.1".to_string(),
            era: EraSettings {
                name: "武侠架空世界".to_string(),
                tech_level: "冷兵器时代，火药仅用于烟火".to_string(),
                social_structure: "封建帝制，江湖与庙堂并存".to_string(),
            },
            allowed_concepts: vec![
                "内力".into(),
                "轻功".into(),
                "武功".into(),
                "点穴".into(),
                "暗器".into(),
                "毒术".into(),
                "医术".into(),
                "易容".into(),
                "阵法".into(),
                "奇门遁甲".into(),
                "相术".into(),
            ],
            forbidden_concepts: vec![
                "魔法".into(),
                "仙术".into(),
                "法术".into(),
                "热武器".into(),
                "现代科技".into(),
                "超能力".into(),
                "异能".into(),
                "穿越".into(),
                "系统".into(),
            ],
            narrative_rules: include_str!("../default_world_rules.md").to_string(),
            last_updated: chrono::Utc::now().to_rfc3339(),
        }
    }
}

impl WorldBuildingRules {
    /// 从 JSON 文件加载规则
    pub fn from_json_file<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| anyhow::anyhow!("Failed to read world rules file: {}", e))?;
        let rules: Self = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse world rules JSON: {}", e))?;
        Ok(rules)
    }
}
