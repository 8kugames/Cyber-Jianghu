// ============================================================================
// OpenClaw Cyber-Jianghu 物品配置访问器
// ============================================================================
//
// 预留：物品系统待集成

use super::global::registry;
use crate::game_data::types::ItemConfigEntry;

/// 物品注册表
#[allow(dead_code)]
pub struct ItemRegistry;

#[allow(dead_code)]
impl ItemRegistry {
    /// 获取指定 item 的完整配置
    pub fn get(item_id: &str) -> Option<ItemConfigEntry> {
        registry().and_then(|r| {
            r.get()
                .items
                .data
                .iter()
                .find(|item| item.item_id == item_id)
                .cloned()
        })
    }

    /// 获取所有已配置的 item ID
    pub fn all_item_ids() -> Vec<String> {
        registry()
            .map(|r| {
                r.get()
                    .items
                    .data
                    .iter()
                    .map(|i| i.item_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 检查指定 item 是否存在
    pub fn exists(item_id: &str) -> bool {
        registry()
            .map(|r| r.get().items.data.iter().any(|i| i.item_id == item_id))
            .unwrap_or(false)
    }
}
