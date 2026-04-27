// ============================================================================
// OpenClaw Cyber-Jianghu 统一配置包装类型
// ============================================================================
//
// 本模块定义统一的配置文件结构，所有配置文件都遵循此格式
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// 统一配置包装结构
// ============================================================================

/// 统一配置包装结构
///
/// 所有配置文件都应遵循此格式：
/// ```json
/// {
///   "version": "版本号",
///   "description": "配置描述",
///   "meta": {
///     "created_at": "创建日期",
///     "updated_at": "更新日期",
///     "author": "作者",
///     "tags": ["标签1", "标签2"]
///   },
///   "data": {
///     // 实际配置数据
///   }
/// }
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UnifiedConfig<T> {
    /// 配置版本号
    pub version: String,

    /// 配置描述
    #[serde(default)]
    pub description: String,

    /// 元数据
    #[serde(default)]
    pub meta: ConfigMeta,

    /// 实际配置数据
    pub data: T,
}

// ============================================================================
// 配置元数据
// ============================================================================

/// 配置元数据
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ConfigMeta {
    /// 创建时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,

    /// 更新时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,

    /// 作者
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// 标签
    #[serde(default)]
    pub tags: Vec<String>,

    /// 额外扩展字段
    #[serde(default)]
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// ============================================================================
// 辅助函数（预留：配置构建器模式）
// ============================================================================

#[allow(dead_code)]
impl<T> UnifiedConfig<T> {
    /// 创建新的统一配置
    pub fn new(version: impl Into<String>, description: impl Into<String>, data: T) -> Self {
        Self {
            version: version.into(),
            description: description.into(),
            meta: ConfigMeta::default(),
            data,
        }
    }

    /// 设置元数据
    pub fn with_meta(mut self, meta: ConfigMeta) -> Self {
        self.meta = meta;
        self
    }

    /// 获取数据引用
    pub fn data(&self) -> &T {
        &self.data
    }

    /// 获取数据可变引用
    pub fn data_mut(&mut self) -> &mut T {
        &mut self.data
    }

    /// 转换为数据
    pub fn into_data(self) -> T {
        self.data
    }
}

// ============================================================================
// 默认元数据生成器（预留：配置构建器模式）
// ============================================================================

#[allow(dead_code)]
impl ConfigMeta {
    /// 创建默认元数据
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置创建时间
    pub fn with_created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = Some(created_at.into());
        self
    }

    /// 设置更新时间
    pub fn with_updated_at(mut self, updated_at: impl Into<String>) -> Self {
        self.updated_at = Some(updated_at.into());
        self
    }

    /// 设置作者
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// 添加标签
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// 添加额外字段
    pub fn with_extra(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

// ============================================================================
// 各配置类型的 Data 结构定义
// ============================================================================

/// 游戏规则配置数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GameRulesData {
    /// Agent状态配置
    pub agent_state: AgentStateRulesData,

    /// Agent状态定义（数据驱动）
    #[serde(default)]
    pub agent_statuses: std::collections::HashMap<String, AgentStatusConfig>,

    /// 验证配置
    pub validation: ValidationRulesData,

    /// 运维与监控配置
    pub ops: OpsRulesData,

    /// 死亡默认配置（当属性未配置 death_cause/death_message 时使用）
    #[serde(default)]
    pub death_defaults: Option<DeathDefaultsData>,

    /// 即时事件配置
    #[serde(default)]
    pub immediate_events: Option<cyber_jianghu_protocol::ImmediateEventConfig>,

    /// Intent批次配置（multi-Intent Pipeline执行）
    #[serde(default)]
    pub intent_batch: Option<cyber_jianghu_protocol::IntentBatchConfig>,

    /// 涌现配置（跨 tick 动作观察）
    #[serde(default)]
    pub emergence: Option<EmergenceConfig>,

    /// 技能配置
    #[serde(default)]
    pub skills: Option<SkillsConfig>,

    /// Vendor 自动补货配置（DEPRECATED: 已迁移到 DB agent_vendor_refill 表）
    #[serde(default, skip_serializing)]
    pub vendors: Vec<VendorConfig>,

    /// 群像传记配置
    #[serde(default)]
    pub chronicle: Option<ChronicleRulesData>,

    /// 寿命配置（数据驱动，下发到 Agent）
    #[serde(default)]
    pub lifespan: Option<cyber_jianghu_protocol::LifespanRules>,

    /// 跨 Agent 传承教训配置
    #[serde(default)]
    pub lesson: Option<LessonConfig>,
}

/// 死因到建议文本的映射（数据驱动，来自 game_rules.yaml）
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct CauseAdvice {
    /// 死因中文名
    pub label: String,
    /// 建议文本
    pub advice: String,
}

/// 教训提取配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LessonConfig {
    /// 同一死因累计多少次死亡后生成教训
    #[serde(default = "default_lesson_threshold")]
    pub threshold: u32,

    /// WorldState 下发最多几条教训
    #[serde(default = "default_lesson_max_broadcast")]
    pub max_broadcast: u32,

    /// 死因 → 建议 映射（数据驱动，替代硬编码 cause_to_advice）
    #[serde(default = "default_cause_advice_map")]
    pub cause_advice_map: std::collections::HashMap<String, CauseAdvice>,
}

fn default_lesson_threshold() -> u32 {
    3
}
fn default_lesson_max_broadcast() -> u32 {
    5
}

impl Default for LessonConfig {
    fn default() -> Self {
        Self {
            threshold: default_lesson_threshold(),
            max_broadcast: default_lesson_max_broadcast(),
            cause_advice_map: default_cause_advice_map(),
        }
    }
}

fn default_cause_advice_map() -> std::collections::HashMap<String, CauseAdvice> {
    let mut m = std::collections::HashMap::new();
    m.insert("hunger".into(), CauseAdvice { label: "饥饿".into(), advice: "请留意饱食度，及时进食".into() });
    m.insert("thirst".into(), CauseAdvice { label: "口渴".into(), advice: "请留意水分，及时饮水".into() });
    m.insert("hp".into(), CauseAdvice { label: "外伤".into(), advice: "请避免危险区域，注意安全".into() });
    m.insert("old_age".into(), CauseAdvice { label: "寿终正寝".into(), advice: "自然规律，无人可免".into() });
    m.insert("environmental".into(), CauseAdvice { label: "环境".into(), advice: "请注意天气和环境影响".into() });
    m
}

/// Vendor 自动补货配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VendorConfig {
    /// Agent 名称（匹配 agent_name）
    pub agent_name: String,
    /// 库存补货规则
    #[serde(default)]
    pub inventory_refill: Vec<VendorRefillRule>,
}

/// 单个 Vendor 补货规则
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VendorRefillRule {
    /// 物品 ID
    pub item_id: String,
    /// 低于此阈值时触发补货
    pub threshold: i32,
    /// 单次最大购买量（受银两预算约束，实际购买 ≤ min(refill_to, 银两/2)）
    pub refill_to: i32,
}

/// 涌现配置：控制 Agent 能观察到的其他 Agent 行为历史
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmergenceConfig {
    /// 回溯多少个 tick 的动作历史
    #[serde(default = "default_recent_action_ticks")]
    pub recent_action_ticks: i64,

    /// 每个实体最多展示多少条最近动作
    #[serde(default = "default_max_recent_actions_per_entity")]
    pub max_recent_actions_per_entity: usize,
}

fn default_recent_action_ticks() -> i64 {
    5
}

fn default_max_recent_actions_per_entity() -> usize {
    5
}

impl Default for EmergenceConfig {
    fn default() -> Self {
        Self {
            recent_action_ticks: default_recent_action_ticks(),
            max_recent_actions_per_entity: default_max_recent_actions_per_entity(),
        }
    }
}

/// 技能配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SkillsConfig {
    /// 单个 Agent 可掌握的技能上限
    #[serde(default = "default_max_skills_per_agent")]
    pub max_skills_per_agent: usize,
}

fn default_max_skills_per_agent() -> usize {
    10
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            max_skills_per_agent: default_max_skills_per_agent(),
        }
    }
}

/// Agent 状态配置（数据驱动）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentStatusConfig {
    /// 显示名称
    pub display_name: String,
    /// 描述
    pub description: String,
    /// 颜色（十六进制）
    pub color: String,
    /// 排序顺序
    pub sort_order: i32,
}

/// 运维与监控规则数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpsRulesData {
    /// 单个 Tick 自然死亡人数告警阈值
    pub death_threshold: usize,

    /// 离线多久（天）的 Agent 会被清理脚本删除
    pub offline_cleanup_days: i32,
}

/// 群像传记规则数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChronicleRulesData {
    /// 关键事件每个类型的最大数量
    #[serde(default = "default_highlight_threshold")]
    pub highlight_threshold: i32,

    /// 传记生成周期（游戏日）
    #[serde(default = "default_days_per_period")]
    pub days_per_period: i32,
}

fn default_highlight_threshold() -> i32 {
    3
}

fn default_days_per_period() -> i32 {
    7
}

impl Default for ChronicleRulesData {
    fn default() -> Self {
        Self {
            highlight_threshold: default_highlight_threshold(),
            days_per_period: default_days_per_period(),
        }
    }
}

/// 死亡默认配置（当属性未配置 death_cause/death_message 时使用）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeathDefaultsData {
    /// 未知原因死亡的默认配置
    pub unknown: DeathDefaultEntry,

    /// 环境伤害死亡的默认配置
    pub environmental: DeathDefaultEntry,

    /// 寿终正寝死亡的默认配置
    #[serde(default = "default_old_age_death")]
    pub old_age: DeathDefaultEntry,
}

fn default_old_age_death() -> DeathDefaultEntry {
    DeathDefaultEntry {
        cause: "old_age".to_string(),
        message: "你已寿终正寝，安详离世......".to_string(),
    }
}

/// 单个死亡默认配置项
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeathDefaultEntry {
    /// 死亡原因代码
    pub cause: String,

    /// 死亡描述
    pub message: String,
}

/// Agent状态规则数据
///
/// 注意：属性衰减和限制已移至 attributes.json 配置
/// 此处只保留全局游戏规则
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentStateRulesData {
    /// Tick配置
    pub tick: TickRulesData,

    /// 生存底线配置
    #[serde(default)]
    pub survival: SurvivalRulesData,

    /// 位置配置
    pub location: LocationRulesData,

    /// 游戏时间配置
    pub game_time: GameTimeRulesData,
}

/// 生存底线规则数据
///
/// 天道无为：服务器不干预 Agent 生存决策，仅提供物理规则（衰减、伤害、死亡）。
/// 所有生存阈值/警告注入已移除，Agent 通过 WorldState.attribute_descriptions（体感叙事）
/// 自主感知状态并做出决策。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SurvivalRulesData {
    /// 自动重生配置
    #[serde(default)]
    pub rebirth: RebirthRulesData,
}

/// 自动重生规则数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RebirthRulesData {
    /// 死亡后延迟 N 个 tick 自动重生 (0 = 不自动重生)
    #[serde(default)]
    pub delay_ticks: i32,

    /// 是否重置属性到初始值
    #[serde(default = "default_true")]
    pub reset_attributes: bool,

    /// 重生地点 (空字符串 = 使用 spawn_location)
    #[serde(default)]
    pub spawn_location: String,
}

impl Default for RebirthRulesData {
    fn default() -> Self {
        Self {
            delay_ticks: 0,
            reset_attributes: true,
            spawn_location: String::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Tick规则数据（现实时间 → Tick 转换）
///
/// 注意：Tick → 游戏时间转换由 time.yaml 的 ticks_per_hour 控制
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TickRulesData {
    /// 服务器每多少秒执行一个 tick
    pub real_seconds_per_tick: i32,
}

/// 位置规则数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocationRulesData {
    pub spawn_location: String,

    /// parent-child 隐式连接的默认 travel_cost
    #[serde(default = "default_implicit_travel_cost")]
    pub default_implicit_travel_cost: u32,
}

fn default_implicit_travel_cost() -> u32 {
    1
}

/// 游戏时间规则数据（用于计算 tick_id）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GameTimeRulesData {
    /// 游戏纪元起始日期（格式：YYYY-MM-DD）
    pub start_date: String,

    /// 时区偏移量（UTC+8 = 8，UTC-5 = -5）
    /// 用于将 start_date 解释为当地时区的午夜
    #[serde(default = "default_timezone_offset")]
    pub timezone_offset: i32,
}

fn default_timezone_offset() -> i32 {
    8 // 默认使用 UTC+8（北京时间）
}

/// 验证规则数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ValidationRulesData {
    pub action_validation: ActionValidationRulesData,
    #[serde(default = "default_max_agent_name_length")]
    pub max_agent_name_length: usize,
    #[serde(default = "default_max_system_prompt_length")]
    pub max_system_prompt_length: usize,
    #[serde(default = "default_max_speak_content_length")]
    pub max_speak_content_length: usize,
}

/// 动作验证规则数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActionValidationRulesData {
    pub max_content_length: usize,
}

/// 时间配置数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TimeData {
    pub ticks_per_hour: i32,
    pub hours_per_day: i32,
    pub days_per_season: i32,
    pub seasons_per_year: i32,
    pub seasons: Vec<SeasonData>,
}

/// 季节数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SeasonData {
    pub id: String,
    pub name: String,
    pub description: String,
    pub temperature_modifier: i32,
    pub resource_growth_rate: f32,
    /// 属性衰减/恢复修饰系数
    /// 键为属性名称（如 "hunger", "thirst", "stamina_recovery"）
    /// 值为修饰系数（1.0 = 无修饰，>1.0 = 增加，<1.0 = 减少）
    /// 例如：{"hunger": 1.5, "thirst": 1.5} 表示冬季饥饿/口渴消耗增加 50%
    #[serde(default)]
    pub attribute_modifiers: std::collections::HashMap<String, f32>,
    /// 天气池：该季节可能出现的天气类型列表
    /// 数组长度决定概率权重（如 ["sunny", "sunny", "stormy"] 中 sunny 概率 2/3）
    #[serde(default)]
    pub weather_pool: Vec<String>,
}

/// 位置配置数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocationsData {
    pub nodes: Vec<LocationNodeData>,
    pub edges: Vec<LocationEdgeData>,
}

/// 位置节点数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocationNodeData {
    pub node_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub parent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environmental_damage: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gatherable_items: Option<Vec<String>>,
    /// 别名列表（供 LLM 别名映射使用）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// 位置边数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocationEdgeData {
    #[serde(rename = "from_node_id")]
    pub from: String,
    #[serde(rename = "to_node_id")]
    pub to: String,
    pub travel_cost: i32,
}

/// 背包限制数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InventoryLimitsData {
    pub max_slots: i32,
    pub max_stack_size: i32,
}

/// 网络配置数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NetworkConfigData {
    pub websocket: WebSocketConfigData,
    #[serde(default)]
    pub dialogue: DialogueConfigData,
}

/// WebSocket配置数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebSocketConfigData {
    pub rate_limit_ms: u64,
    pub cleanup_interval_secs: u64,
    pub cleanup_threshold: usize,
    /// WebSocket 消息通道容量（背压控制）
    #[serde(default = "default_channel_capacity")]
    pub channel_capacity: usize,
    /// 心跳 Ping 间隔（秒）
    #[serde(default = "default_heartbeat_interval_secs")]
    pub heartbeat_interval_secs: u64,
    /// 连续未收到 Pong 的最大次数（超过则断连）
    #[serde(default = "default_max_missed_pongs")]
    pub max_missed_pongs: u8,
    /// 日志消息预览截断长度
    #[serde(default = "default_log_preview_length")]
    pub log_preview_length: usize,
}

fn default_channel_capacity() -> usize {
    100
}
fn default_heartbeat_interval_secs() -> u64 {
    30
}
fn default_max_missed_pongs() -> u8 {
    3
}
fn default_log_preview_length() -> usize {
    50
}

/// 对话配置数据
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DialogueConfigData {
    #[serde(default = "default_dialogue_window")]
    pub window_seconds: u64,
    #[serde(default = "default_max_messages")]
    pub max_messages_per_agent: u32,
}

/// 世界构建规则数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorldBuildingRulesData {
    pub era: EraData,
    pub allowed_concepts: Vec<String>,
    pub forbidden_concepts: Vec<String>,
    pub narrative_rules: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
}

/// 时代数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EraData {
    pub name: String,
    pub tech_level: String,
    pub social_structure: String,
}

/// 叙事配置数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NarrativeData {
    pub attributes: std::collections::HashMap<String, AttributeNarrativeData>,
    pub status_effects: std::collections::HashMap<String, StatusEffectNarrativeData>,
}

/// 属性叙事数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AttributeNarrativeData {
    pub name: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub thresholds: Vec<ThresholdData>,
}

/// 阈值数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ThresholdData {
    pub min: i32,
    pub max: i32,
    pub description: String,
}

/// 状态效果叙事数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatusEffectNarrativeData {
    pub description: String,
}

// ============================================================================
// 辅助默认函数
// ============================================================================

fn default_max_agent_name_length() -> usize {
    100
}
fn default_max_system_prompt_length() -> usize {
    102400
}
fn default_max_speak_content_length() -> usize {
    500
}
fn default_dialogue_window() -> u64 {
    30
}
fn default_max_messages() -> u32 {
    10
}

// ============================================================================
// 旧类型别名（兼容性）
// ============================================================================

/// 物品配置数据
pub type ItemsData = Vec<crate::game_data::types::ItemConfigEntry>;

/// 配方配置数据
pub type RecipesData = std::collections::HashMap<String, crate::game_data::types::RecipeDefinition>;

/// 动作配置数据
pub type ActionsData =
    std::collections::HashMap<String, crate::game_data::types::ActionConfigEntry>;

// ============================================================================
// 统一类型别名
// ============================================================================

/// 统一物品配置
pub type UnifiedItemsConfig = UnifiedConfig<ItemsData>;

/// 统一配方配置
pub type UnifiedRecipesConfig = UnifiedConfig<RecipesData>;

/// 统一游戏规则配置
pub type UnifiedGameRulesConfig = UnifiedConfig<GameRulesData>;

/// 统一时间配置
pub type UnifiedTimeConfig = UnifiedConfig<TimeData>;

/// 统一位置配置
pub type UnifiedLocationsConfig = UnifiedConfig<LocationsData>;

/// 统一动作配置
pub type UnifiedActionsConfig = UnifiedConfig<ActionsData>;

/// 统一背包配置
pub type UnifiedInventoryConfig = UnifiedConfig<InventoryLimitsData>;

/// 统一网络配置
pub type UnifiedNetworkConfig = UnifiedConfig<NetworkConfigData>;

/// 统一初始背包配置
pub type UnifiedInitialInventoryConfig = UnifiedConfig<super::inventory::InitialInventoryData>;

/// 统一世界构建规则配置
pub type UnifiedWorldBuildingRulesConfig = UnifiedConfig<WorldBuildingRulesData>;

/// 统一叙事配置
pub type UnifiedNarrativeConfig = UnifiedConfig<NarrativeData>;

// 注意：UnifiedAttributesConfig 在 unified_attributes.rs 中定义，避免循环依赖

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_config() {
        let data = InventoryLimitsData {
            max_slots: 10,
            max_stack_size: 99,
        };
        let config = UnifiedConfig::new("0.0.1", "Test config", data);

        assert_eq!(config.version, "0.0.1");
        assert_eq!(config.description, "Test config");
        assert_eq!(config.data.max_slots, 10);
    }

    #[test]
    fn test_config_meta() {
        let meta = ConfigMeta::new()
            .with_created_at("2026-03-16")
            .with_author("System")
            .with_tag("test");

        assert_eq!(meta.created_at, Some("2026-03-16".to_string()));
        assert_eq!(meta.author, Some("System".to_string()));
        assert_eq!(meta.tags, vec!["test"]);
    }

    #[test]
    fn test_unified_config_serialization() {
        let data = InventoryLimitsData {
            max_slots: 20,
            max_stack_size: 50,
        };
        let config = UnifiedConfig::new("0.0.1", "Test", data);
        let json = serde_json::to_string(&config).unwrap();
        let parsed: UnifiedConfig<InventoryLimitsData> = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, "0.0.1");
        assert_eq!(parsed.data.max_slots, 20);
    }

    #[test]
    fn test_time_data_serialization() {
        let data = TimeData {
            ticks_per_hour: 1,
            hours_per_day: 24,
            days_per_season: 10,
            seasons_per_year: 4,
            seasons: vec![],
        };
        let config = UnifiedConfig::new("0.0.1", "Time config", data);
        let json = serde_json::to_string(&config).unwrap();
        let parsed: UnifiedConfig<TimeData> = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, "0.0.1");
        assert_eq!(parsed.data.ticks_per_hour, 1);
    }
}
