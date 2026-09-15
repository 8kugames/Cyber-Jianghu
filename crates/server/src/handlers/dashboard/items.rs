use axum::Json;
use serde::Serialize;

// ============================================================================
// Items API (物品列表，供 Admin 面板 grant-items UI 使用)
// ============================================================================

/// 物品摘要（用于下拉选择器）
#[derive(Debug, Serialize)]
pub struct ItemSummary {
    pub item_id: String,
    /// 稳定 uuid（UUID v5 从 item_id 派生，展示用短 uuid 取前 8 位）
    pub uuid: String,
    pub name: String,
    pub item_type: String,
    pub description: String,
}

/// 获取所有已配置物品列表
///
/// GET /api/dashboard/items
pub async fn get_items() -> Json<Vec<ItemSummary>> {
    let items = crate::game_data::registry::ItemRegistry::all_item_ids()
        .iter()
        .filter_map(|id| crate::game_data::registry::ItemRegistry::get(id))
        .map(|entry| {
            let uuid = crate::items::item_uuid(&entry.item_id).to_string();
            ItemSummary {
                uuid,
                item_id: entry.item_id,
                name: entry.name,
                item_type: entry.item_type,
                description: entry.description,
            }
        })
        .collect();
    Json(items)
}
