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

/// 死亡默认配置（当属性未配置 death_cause/death_message 时使用）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeathDefaultsData {
    /// 未知原因死亡的默认配置
    pub unknown: DeathDefaultEntry,

    /// 环境伤害死亡的默认配置
    pub environmental: DeathDefaultEntry,
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

    /// 位置配置
    pub location: LocationRulesData,

    /// 游戏时间配置
    pub game_time: GameTimeRulesData,
}

/// Tick规则数据（现实时间 → Tick 转换）
///
/// 注意：Tick → 游戏时间转换由 time.yaml 的 ticks_per_hour 控制
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TickRulesData {
    /// 服务器每多少秒执行一个 tick
    pub real_seconds_per_tick: i32,

    /// 收集窗口时长（秒）：每个 tick 周期开始后，等待此时间再执行 tick
    /// 用于收集 Agent 意图，避免意图因时序错位而丢失
    /// 设为 0 可禁用收集窗口
    #[serde(default = "default_collection_window_secs")]
    pub collection_window_secs: u32,
}

/// 位置规则数据
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocationRulesData {
    pub spawn_location: String,
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
fn default_collection_window_secs() -> u32 {
    5
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
            ticks_per_hour: 60,
            hours_per_day: 24,
            days_per_season: 10,
            seasons: vec![],
        };
        let config = UnifiedConfig::new("0.0.1", "Time config", data);
        let json = serde_json::to_string(&config).unwrap();
        let parsed: UnifiedConfig<TimeData> = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.version, "0.0.1");
        assert_eq!(parsed.data.ticks_per_hour, 60);
    }
}
