// ============================================================================
// 库存/地面物品查询（注册握手期下发）
// ============================================================================

use super::*;

/// 加载并规范化 Agent 初始背包物品。
///
/// 之前在 DB 失败时静默回退为 `vec![]`，会让 Agent 在"空背包"假状态下决策。
/// 现改为：任何错误（DB 抖动、列漂移、Schema 异常）必须显式返回 Err，
/// 由 caller 决定是否关闭连接。
pub(crate) async fn load_initial_inventory(
    db_pool: &sqlx::PgPool,
    agent_id: uuid::Uuid,
) -> anyhow::Result<Vec<crate::models::InventoryItem>> {
    let raw_items = InventoryManager::get_all_items(db_pool, agent_id)
        .await
        .context("query agent inventory")?;
    Ok(raw_items
        .into_iter()
        .map(|item| {
            let config = ItemRegistry::get(&item.item_id);
            let name = config
                .as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| item.item_id.clone());
            let item_type = config
                .as_ref()
                .map(|c| c.item_type.clone())
                .unwrap_or_default();
            crate::models::InventoryItem {
                // 协议层携带物品 uuid（v5 派生），动作边界反解
                item_id: crate::items::item_uuid(&item.item_id).to_string(),
                name,
                quantity: item.quantity,
                is_equipped: item.is_equipped,
                item_type,
            }
        })
        .collect())
}

/// 加载并规范化当前节点地面物品。
pub(crate) async fn load_nearby_ground_items(
    db_pool: &sqlx::PgPool,
    node_id: &str,
) -> anyhow::Result<Vec<cyber_jianghu_protocol::SceneItem>> {
    let raw_items = crate::db::get_ground_items_by_node(db_pool, node_id)
        .await
        .context("query ground items")?;
    Ok(raw_items
        .into_iter()
        .map(|gi| {
            let config = ItemRegistry::get(&gi.item_id);
            let name = config
                .as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| gi.item_id.clone());
            let item_type = config
                .as_ref()
                .map(|c| c.item_type.clone())
                .unwrap_or_default();
            cyber_jianghu_protocol::SceneItem {
                item_id: crate::items::item_uuid(&gi.item_id).to_string(),
                name,
                quantity: gi.quantity,
                item_type,
            }
        })
        .collect())
}
