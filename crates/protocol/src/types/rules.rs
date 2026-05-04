//! 游戏规则相关类型
//!
//! 包含游戏规则和世界观规则
//!
//! # 设计哲学
//!
//! 本协议 crate 为 Cyber-Jianghu 武侠 MMO 设计。
//!
//! ## 数据驱动 + Fail-Fast
//!
//! 所有业务配置必须通过 YAML 配置文件加载，不允许在协议层硬编码业务默认值。
//! 配置缺失时 loader 返回 `Err`，确保配置错误在启动/测试阶段暴露，而非运行时静默回退。
//!
//! ## 配置来源
//!
//! - `WorldBuildingRules` 由 server 下发，通过 `ServerMessage::Registered` 或 `ConfigUpdate` 到达 agent
//! - `GameRules` 由 server 从 `game_rules.yaml` 加载并广播
//! - 测试 fixtures 必须提供完整的配置文件（参见 `test_utils.rs`）

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// 即时事件处理配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immediate_events: Option<ImmediateEventConfig>,

    /// 死亡后自动重生延迟 tick 数（0 = 不自动重生）
    #[serde(default)]
    pub rebirth_delay_ticks: i32,

    /// 自动重生重试次数
    pub rebirth_retry_max_attempts: u32,

    /// 自动重生重试间隔（秒）
    pub rebirth_retry_interval_secs: u64,

    /// 寿命配置（可选，从 game_rules.yaml 下发）
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
pub struct DailySummaryConfig {
    /// 发送失败时的最大重试次数
    pub max_retries: u32,
    /// 超过此 tick 数后丢弃（防止跨 game_day 重试）
    pub ttl_ticks: i64,
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
    pub max_age: u8,

    /// 衰老开始年龄（影响叙事描述）
    pub aging_start_age: u8,

    /// 新角色/重生角色的初始年龄（岁）
    pub starting_age: u8,
}

// ============================================================================
// Multi-Intent 配置
// ============================================================================

/// Intent 批次配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentBatchConfig {
    /// 每 tick 最大 Intent 数
    pub max_intents_per_tick: usize,

    /// 三魂循环最大重试次数
    pub max_retries: i32,

    /// 是否启用 Pipeline 执行
    pub pipeline_execution_enabled: bool,

    /// 是否允许部分执行
    pub partial_execution_enabled: bool,

    /// 分级审核配置
    pub llm_validation: GradedValidationConfig,

    /// LLM 连续失败多少 tick 后激活 chaos 模式
    pub llm_chaos_threshold: u32,
}

/// 分级审核配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradedValidationConfig {
    /// 强制 LLM 审核的 action_type（speak/shout/whisper 等）
    pub always_types: Vec<String>,

    /// 动态审核的 action_type（根据 action_data 判断）
    pub adaptive_types: Vec<String>,

    /// 跳过 LLM 审核的 action_type
    pub skip_types: Vec<String>,

    /// 每 tick 至少审核的 Intent 数量
    pub minimum_per_tick: usize,

    /// 限制区域 node_id 前缀/关键词（move 审核用）
    pub restricted_area_keywords: Vec<String>,

    /// 高价值物品 item_id 前缀/关键词（trade/steal/give 审核用）
    pub high_value_item_keywords: Vec<String>,

    /// Adaptive 审核字段映射（数据驱动）
    ///
    /// 格式: { "action_type": "action_data_field_name" }
    /// 例如: { "移动": "target_location", "偷窃": "item_id", "给予": "item_id" }
    /// 当 action_type 在此映射中时，检查对应字段的值是否匹配对应的关键词列表：
    /// - "target_location" → 检查 restricted_area_keywords
    /// - "item_id" → 检查 high_value_item_keywords
    /// - 其他字段 → 默认需要 LLM 审核
    pub adaptive_field_mapping: std::collections::HashMap<String, String>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub daily_summary_importance: f64,
}

/// SQL 预筛配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventTriagePreFilter {
    /// 每次 triage 最多处理 N 条
    pub max_events_per_triage: usize,

    /// 未配置事件类型的默认权重
    pub default_priority: i32,

    /// SQL ORDER BY 权重（仅用于预筛排序）
    pub event_type_priority: HashMap<WorldEventType, i32>,

    /// 兜底分流：urgent 阈值（priority >= threshold）
    pub fallback_urgent_cutoff_priority: i32,

    /// 兜底分流：ignored 阈值（priority < threshold）
    pub fallback_ignore_cutoff_priority: i32,
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
            max_events_per_triage: 50,
            default_priority: 0,
            event_type_priority: Default::default(),
        };
        assert!(pre.fallback_thresholds().is_err());
    }
}

/// 主 tick 上下文注入配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventTriageContext {
    /// 逐条注入的最大 urgent 事件数
    pub max_urgent_events: usize,

    /// batch 摘要最大字符数
    pub max_batch_summary_chars: usize,
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
