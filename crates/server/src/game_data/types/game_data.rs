// ============================================================================
// OpenClaw Cyber-Jianghu 游戏数据聚合结构体
// ============================================================================
//
// 本模块包含 GameData 聚合结构体
// ============================================================================

use super::display_messages::DisplayMessagesConfig;
use super::unified_attributes::UnifiedAttributesConfig;
use super::unified_config::*;
use cyber_jianghu_protocol::NarrativeConfig;

/// 游戏数据聚合结构体
///
/// 包含所有游戏配置的集合，用于缓存和加载
#[derive(Debug, Clone)]
pub struct GameData {
    /// 游戏规则配置
    pub game_rules: UnifiedGameRulesConfig,

    /// 物品配置
    pub items: UnifiedItemsConfig,

    /// 动作配置
    pub actions: UnifiedActionsConfig,

    /// 初始物品配置
    pub initial_inventory: UnifiedInitialInventoryConfig,

    /// 背包配置
    pub inventory: UnifiedInventoryConfig,

    /// 网络配置
    pub network: UnifiedNetworkConfig,

    /// 位置配置
    pub locations: UnifiedLocationsConfig,

    /// 统一属性配置（包含所有属性定义）
    pub attributes: UnifiedAttributesConfig,

    /// 配方配置 (recipes.json)
    pub recipes: UnifiedRecipesConfig,

    /// 时间与季节配置 (time.json)
    pub time: UnifiedTimeConfig,

    /// 叙事化配置 (narrative_config.json)
    /// 用于将数值属性转换为自然语言描述
    pub narrative: NarrativeConfig,

    /// 显示消息配置 (display_messages.yaml)
    /// 数据驱动的 UI 显示消息
    pub display_messages: DisplayMessagesConfig,
}
