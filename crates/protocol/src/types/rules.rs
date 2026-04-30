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

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::world::WorldEventType;

use super::entities::{AvailableAction, InitialItem};

/// 游戏规则
///
/// 服务端下发的游戏规则配置，包含可用动作和初始物品
///
/// 天道无为：survival_threshold / critical_attack_threshold / hp_critical / hp_force_flee
/// 等干预字段已移除。Agent 通过 WorldState.attribute_descriptions（体感叙事）自主感知状态。
fn default_rebirth_retry_max() -> u32 {
    3
}
fn default_rebirth_retry_interval() -> u64 {
    30
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GameRules {
    /// Tick 周期（秒）
    pub tick_duration_secs: u64,

    /// 可用动作列表
    pub available_actions: Vec<AvailableAction>,

    /// 初始物品（注册时发放）
    pub initial_items: Vec<InitialItem>,

    /// 生存相关动作列表（hunger/thirst 低于阈值时绕过 ReflectorSoul 审查）
    /// 保留用于 ReflectorSoul 分级审核路由，非 Agent 端干预
    #[serde(default)]
    pub survival_actions: Vec<String>,

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

    /// 自动重生重试次数
    #[serde(default = "default_rebirth_retry_max")]
    pub rebirth_retry_max_attempts: u32,

    /// 自动重生重试间隔（秒）
    #[serde(default = "default_rebirth_retry_interval")]
    pub rebirth_retry_interval_secs: u64,

    /// 寿命配置（可选，不配置则使用 LifespanRules 默认值）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifespan: Option<LifespanRules>,

    /// 日历配置（可选，从 time.yaml 下发，Agent 用于计算 game_day）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<CalendarConfig>,

    /// 每日 LLM 日志摘要提交配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_summary: Option<DailySummaryConfig>,
}

/// 每日摘要提交配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DailySummaryConfig {
    /// 发送失败时的最大重试次数
    pub max_retries: u32,
    /// 超过此 tick 数后丢弃（防止跨 game_day 重试）
    pub ttl_ticks: i64,
}

impl Default for DailySummaryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            ttl_ticks: 10080, // 7 游戏日 × 24h × 60m = 10080 ticks（假设 ticks_per_hour=1）
        }
    }
}

impl Default for GameRules {
    fn default() -> Self {
        Self {
            tick_duration_secs: 60,
            available_actions: Vec::new(),
            initial_items: Vec::new(),
            survival_actions: Vec::new(),
            version: "0.0.1".into(),
            last_updated: chrono::Utc::now().to_rfc3339(),
            intent_batch: None,
            reflector_narrative: None,
            immediate_events: None,
            rebirth_delay_ticks: 0,
            rebirth_retry_max_attempts: default_rebirth_retry_max(),
            rebirth_retry_interval_secs: default_rebirth_retry_interval(),
            lifespan: None,
            calendar: None,
            daily_summary: None,
        }
    }
}

/// 日历配置（数据驱动，从 time.yaml 下发）
///
/// Agent 用于从 WorldTime 计算 game_day（单调递增天数），
/// 避免 Agent 端硬编码日历参数。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalendarConfig {
    /// 每季节天数
    pub days_per_season: u32,
    /// 每年季节数
    pub seasons_per_year: u32,
}

/// 寿命数据驱动配置（由 server game_rules.yaml 下发）
///
/// ticks_per_year 从 time.yaml 唯一配置源派生：
/// ticks_per_hour * hours_per_day * days_per_season * seasons_per_year
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LifespanRules {
    /// 角色最大寿命（岁）
    #[serde(default = "default_max_age")]
    pub max_age: u8,

    /// 衰老开始年龄（影响叙事描述）
    #[serde(default = "default_aging_start_age")]
    pub aging_start_age: u8,

    /// 新角色/重生角色的初始年龄（岁）
    #[serde(default = "default_starting_age")]
    pub starting_age: u8,
}

fn default_max_age() -> u8 {
    80
}
fn default_aging_start_age() -> u8 {
    50
}
fn default_starting_age() -> u8 {
    18
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
/// 控制 Agent 如何处理 Server 下发的即时事件（speak/whisper 等）
/// 新架构：EventStore SQLite 持久化 + Session Triage LLM 批量分流
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImmediateEventConfig {
    /// 事件 triage 配置（DB 持久化 + Session LLM 分流）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_triage: Option<EventTriageConfig>,
}

// ============================================================================
// 事件 Triage 配置（数据驱动）
// ============================================================================

/// 事件 triage 配置（数据驱动，由 game_rules.yaml 下发）
///
/// 控制事件摄取→分类→消费的完整生命周期。
/// 所有字段均有默认值，旧配置文件无需修改即可运行。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EventTriageConfig {
    /// Session 生命周期模式（当前仅 "game_day"）
    pub lifecycle: String,

    /// 无事件时的兜底轮询间隔（秒）
    pub poll_interval_secs: u64,

    /// 收到事件后的收集窗口（秒），窗口内事件合并为一次 triage
    pub debounce_secs: u64,

    /// 单次 triage LLM 调用超时（ms）
    pub triage_llm_timeout_ms: u64,

    /// SQL 预筛配置
    pub pre_filter: EventTriagePreFilter,

    /// 主 tick 上下文注入配置
    pub context: EventTriageContext,

    /// 保留最近 N 个游戏日的事件
    pub retention_game_days: u32,

    /// 每日摘要写入 episodic memory 的 importance 值
    #[serde(default = "default_daily_summary_importance")]
    pub daily_summary_importance: f64,
}

fn default_daily_summary_importance() -> f64 {
    0.8
}

impl Default for EventTriageConfig {
    fn default() -> Self {
        Self {
            lifecycle: "game_day".into(),
            poll_interval_secs: 10,
            debounce_secs: 3,
            triage_llm_timeout_ms: 10000,
            pre_filter: EventTriagePreFilter::default(),
            context: EventTriageContext::default(),
            retention_game_days: 3,
            daily_summary_importance: default_daily_summary_importance(),
        }
    }
}

/// SQL 预筛配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EventTriagePreFilter {
    /// 每次 triage 最多处理 N 条
    pub max_events_per_triage: usize,

    /// 未配置事件类型的默认权重
    pub default_priority: i32,

    /// SQL ORDER BY 权重（仅用于预筛排序）
    pub event_type_priority: HashMap<WorldEventType, i32>,

    /// 兜底分流：urgent 阈值（priority >= threshold）
    #[serde(default = "default_fallback_urgent_cutoff_priority")]
    pub fallback_urgent_cutoff_priority: i32,

    /// 兜底分流：ignored 阈值（priority < threshold）
    #[serde(default = "default_fallback_ignore_cutoff_priority")]
    pub fallback_ignore_cutoff_priority: i32,
}

fn default_fallback_urgent_cutoff_priority() -> i32 {
    80
}

fn default_fallback_ignore_cutoff_priority() -> i32 {
    20
}

impl Default for EventTriagePreFilter {
    fn default() -> Self {
        let mut priorities = HashMap::new();
        priorities.insert(WorldEventType::DeathNotification, 100);
        priorities.insert(WorldEventType::PrivateDialogue, 80);
        priorities.insert(WorldEventType::SocialInteraction, 60);
        priorities.insert(WorldEventType::StateChange, 50);
        priorities.insert(WorldEventType::ActionResult, 40);
        priorities.insert(WorldEventType::PublicMessage, 20);
        priorities.insert(WorldEventType::EnvironmentalChange, 10);
        priorities.insert(WorldEventType::SystemNotification, 10);
        priorities.insert(WorldEventType::TimeUpdate, 5);

        Self {
            max_events_per_triage: 50,
            default_priority: 0,
            event_type_priority: priorities,
            fallback_urgent_cutoff_priority: default_fallback_urgent_cutoff_priority(),
            fallback_ignore_cutoff_priority: default_fallback_ignore_cutoff_priority(),
        }
    }
}

impl EventTriagePreFilter {
    pub fn fallback_thresholds(&self) -> Result<(i32, i32), String> {
        let urgent = self.fallback_urgent_cutoff_priority;
        let ignore = self.fallback_ignore_cutoff_priority;
        if ignore >= urgent {
            return Err(format!(
                "invalid fallback thresholds: ignore_cutoff={} must be < urgent_cutoff={}",
                ignore, urgent
            ));
        }
        Ok((urgent, ignore))
    }
}

#[cfg(test)]
mod triage_fallback_threshold_tests {
    use super::EventTriagePreFilter;

    #[test]
    fn fallback_thresholds_reject_invalid_order() {
        let pre = EventTriagePreFilter {
            fallback_urgent_cutoff_priority: 10,
            fallback_ignore_cutoff_priority: 20,
            ..Default::default()
        };
        assert!(pre.fallback_thresholds().is_err());
    }
}

/// 主 tick 上下文注入配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EventTriageContext {
    /// 逐条注入的最大 urgent 事件数
    pub max_urgent_events: usize,

    /// batch 摘要最大字符数
    pub max_batch_summary_chars: usize,
}

impl Default for EventTriageContext {
    fn default() -> Self {
        Self {
            max_urgent_events: 5,
            max_batch_summary_chars: 500,
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
