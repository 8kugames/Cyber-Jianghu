// ============================================================================
// OpenClaw Cyber-Jianghu 背包配置访问器
// ============================================================================

use super::global::registry;
use crate::game_data::types::{InitialInventoryItem, InventoryLimitsData};

/// 初始物品清单配置访问器
pub struct InitialInventoryRegistry;

impl InitialInventoryRegistry {
    /// 获取所有初始物品
    pub fn items() -> Vec<InitialInventoryItem> {
        registry()
            .map(|r| r.get().initial_inventory.data.items.clone())
            .expect("配置未初始化，请确保 initial-inventory.json 已正确加载")
    }

    /// 获取重生发放物品（配置缺省时回退完整初始清单）
    pub fn rebirth_items() -> Vec<InitialInventoryItem> {
        registry()
            .map(|r| {
                let data = &r.get().initial_inventory.data;
                if data.rebirth_items.is_empty() {
                    data.items.clone()
                } else {
                    data.rebirth_items.clone()
                }
            })
            .expect("配置未初始化，请确保 initial-inventory.json 已正确加载")
    }
}

/// 背包配置访问器
pub struct InventoryRegistry;

impl InventoryRegistry {
    /// 获取背包限制配置
    pub fn limits() -> InventoryLimitsData {
        registry()
            .map(|r| r.get().inventory.data.clone())
            .expect("配置未初始化，请确保 inventory.json 已正确加载")
    }
}
