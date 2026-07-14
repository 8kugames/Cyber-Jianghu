use crate::game_data::ItemEffect;
use crate::models::ItemType;

// ============================================================================
// 物品定义
// ============================================================================

/// 物品定义
///
/// 定义物品的基本属性和效果
#[derive(Debug, Clone, PartialEq)]
pub struct ItemDefinition {
    /// 物品唯一ID（如：mantou, water, silver, knife）
    pub item_id: String,

    /// 物品名称（如：馒头、水、银子、刀）
    pub name: String,

    /// 物品类型
    pub item_type: ItemType,

    /// 效果列表
    pub effects: Vec<ItemEffect>,

    /// 物品描述
    pub description: String,

    /// 最大耐久度
    pub max_durability: i32,

    /// 自然衰减速率
    pub decay_rate: i32,
}

impl ItemDefinition {
    /// 创建新的物品定义
    pub fn new(
        item_id: &str,
        name: &str,
        item_type: ItemType,
        effects: Vec<ItemEffect>,
        description: &str,
        max_durability: i32,
        decay_rate: i32,
    ) -> Self {
        Self {
            item_id: item_id.to_string(),
            name: name.to_string(),
            item_type,
            effects,
            description: description.to_string(),
            max_durability,
            decay_rate,
        }
    }

    /// 检查物品是否可使用
    ///
    /// 所有物品均可使用（用/吃/喝），效果由 effects 定义：
    /// - consumable: 正向增益
    /// - material: 轻微正向 + 轻微负向
    /// - currency/weapon/tool: 负向减益（或不允许无效果）
    pub fn is_usable(&self) -> bool {
        true
    }

    /// 检查物品是否可装备（预留：装备系统）
    ///
    /// 只有 Weapon 类型的物品可以装备
    #[allow(dead_code)]
    pub fn is_equippable(&self) -> bool {
        self.item_type == ItemType::Weapon
    }
}
