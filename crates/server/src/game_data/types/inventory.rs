// ============================================================================
// OpenClaw Cyber-Jianghu 数据驱动配置类型定义 - 背包相关
// ============================================================================
//
// 本模块定义背包相关的数据结构，从 legacy.rs 拆分出来
// ============================================================================

use serde::{Deserialize, Serialize};

// ============================================================================
// 初始物品条目
// ============================================================================

/// 初始物品条目
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InitialInventoryItem {
    /// 物品ID
    pub item_id: String,

    /// 物品名称
    pub name: String,

    /// 数量
    pub quantity: i32,

    /// 物品描述
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_inventory_item() {
        let item = InitialInventoryItem {
            item_id: "test".to_string(),
            name: "测试物品".to_string(),
            quantity: 1,
            description: "测试".to_string(),
        };

        assert_eq!(item.item_id, "test");
    }
}
