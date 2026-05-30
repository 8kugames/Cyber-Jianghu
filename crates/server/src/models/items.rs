// ============================================================================
// 物品相关数据结构
// ============================================================================
//
// 注意：物品定义主要来自配置文件 (items.yaml)
// 数据库 items 表用于 FK 约束，数据结构与之对应
// ============================================================================

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// 物品类型（业务逻辑枚举）
///
/// 用于业务逻辑判断，数据库存储为字符串
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    /// 消耗品（如馒头、水）
    Consumable,

    /// 武器（如刀）
    Weapon,

    /// 货币（如银子）
    Currency,

    /// 材料（如面粉、木材）
    Material,

    /// 工具（制作时需要，不消耗）
    Tool,
}

impl ItemType {
    /// 从字符串解析
    pub fn parse(s: &str) -> Self {
        match s {
            "consumable" => ItemType::Consumable,
            "weapon" => ItemType::Weapon,
            "currency" => ItemType::Currency,
            "material" => ItemType::Material,
            "tool" => ItemType::Tool,
            _ => ItemType::Consumable, // 默认值
        }
    }
}

/// 物品效果（数据库模型）
///
/// 与配置文件的 ItemEffect 结构对应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemEffect {
    /// 目标属性
    pub attribute: String,

    /// 操作类型（add/subtract/multiply/set）
    pub operation: String,

    /// 效果值
    pub value: JsonValue,
}
