// ============================================================================
// 物品相关数据结构
// ============================================================================

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// 物品类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum ItemType {
    /// 消耗品（如馒头、水）
    Consumable,

    /// 武器（如刀）
    Weapon,

    /// 货币（如银子）
    Currency,
}

/// 物品效果类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum EffectType {
    /// 恢复饥饿值
    RestoreHunger,

    /// 恢复口渴值
    RestoreThirst,

    /// 增加攻击力
    IncreaseAttack,
}

/// 物品模板
///
/// 定义物品的基本属性和效果
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Item {
    /// 物品唯一ID（如：mantou, water, silver, knife）
    pub item_id: String,

    /// 物品名称（如：馒头、水、银子、刀）
    pub name: String,

    /// 物品类型
    pub item_type: ItemType,

    /// 效果类型（可选）
    pub effect_type: Option<EffectType>,

    /// 效果值（如：馒头恢复饥饿值30点）
    pub effect_value: i32,

    /// 物品描述
    pub description: String,

    /// 最大耐久度（默认 -1 表示无限）
    pub max_durability: i32,

    /// 自然衰减速率（每Tick减少的耐久度，默认 0）
    pub decay_rate: i32,
}

/// Agent背包中的物品
///
/// 记录Agent拥有的物品及数量
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
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
