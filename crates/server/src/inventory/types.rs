// ============================================================================
// OpenClaw Cyber-Jianghu 背包类型
// ============================================================================

use crate::game_data::InventoryRegistry;
use uuid::Uuid;

/// 背包物品
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InventoryItem {
    /// 记录ID
    pub id: i64,
    /// Agent ID
    #[allow(dead_code)]
    pub agent_id: Uuid,
    /// 物品 ID
    pub item_id: String,

    /// 数量
    pub quantity: i32,

    /// 是否已装备
    pub is_equipped: bool,
}

// ============================================================================
// 背包配置访问器（数据驱动）
// ============================================================================

/// 获取最大物品格数
pub fn get_max_slots() -> i32 {
    InventoryRegistry::limits().max_slots
}

/// 获取每格最大堆叠数量
pub fn get_max_stack_size() -> i32 {
    InventoryRegistry::limits().max_stack_size
}
