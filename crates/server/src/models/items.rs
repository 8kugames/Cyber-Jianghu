// ============================================================================
// 物品相关数据结构
// ============================================================================
//
// 注意：物品定义主要来自配置文件 (items.yaml)
// 数据库 items 表用于 FK 约束，数据结构与之对应
// ============================================================================

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::FromRow;
use uuid::Uuid;

/// 物品类型（业务逻辑枚举）
///
/// 用于业务逻辑判断，数据库存储为字符串
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
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
    pub fn from_str(s: &str) -> Self {
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

/// 物品模板（数据库模型）
///
/// 对应 items 表，使用 JSONB 存储 effects 数组
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[allow(dead_code)]
pub struct Item {
    /// 物品唯一ID（如：mantou, water, silver, knife）
    pub item_id: String,

    /// 物品名称（如：馒头、水、银子、刀）
    pub name: String,

    /// 物品类型 (consumable/weapon/currency/material/tool)
    pub item_type: String,

    /// 效果列表（JSONB 数组）
    #[sqlx(json)]
    pub effects: Vec<ItemEffect>,

    /// 可堆叠数量
    pub stack_size: i32,

    /// 物品描述
    pub description: Option<String>,
}

/// 物品效果（数据库模型）
///
/// 与配置文件的 ItemEffect 结构对应
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ItemEffect {
    /// 目标属性
    pub attribute: String,

    /// 操作类型（add/subtract/multiply/set）
    pub operation: String,

    /// 效果值
    pub value: JsonValue,
}

/// Agent背包中的物品（预留：背包物品查询）
///
/// 记录Agent拥有的物品及数量
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[allow(dead_code)]
pub struct AgentItem {
    /// 记录ID
    pub id: i64,

    /// Agent ID
    pub agent_id: Uuid,

    /// 物品ID
    pub item_id: String,

    /// 物品数量（同类物品可堆叠，每格最多10个）
    pub quantity: i32,

    /// 是否已装备（仅对武器有效）
    pub is_equipped: bool,

    /// 当前耐久度
    pub durability: i32,

    /// 创建时间
    pub created_at: DateTime<Utc>,

    /// 更新时间
    pub updated_at: DateTime<Utc>,
}
