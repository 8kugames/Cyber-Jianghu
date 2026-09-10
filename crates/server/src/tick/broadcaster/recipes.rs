// ============================================================================
// 配方详情构建（自 broadcaster.rs 拆分）
// ============================================================================

/// 从已知配方 ID 列表构建 RecipeDetail 列表
pub fn build_recipe_details(
    known_recipe_ids: &[String],
) -> Vec<cyber_jianghu_protocol::types::entities::RecipeDetail> {
    known_recipe_ids
        .iter()
        .filter_map(|recipe_id| {
            let recipe = crate::game_data::registry::RecipeRegistry::get(recipe_id)?;
            let result_item_config =
                crate::game_data::registry::ItemRegistry::get(&recipe.result_item);
            let materials: Vec<cyber_jianghu_protocol::types::entities::RecipeMaterialInfo> =
                recipe
                    .materials
                    .iter()
                    .map(|m| {
                        let item_config = crate::game_data::registry::ItemRegistry::get(&m.item_id);
                        cyber_jianghu_protocol::types::entities::RecipeMaterialInfo {
                            // 材料引用 uuid（与背包/地面物品同标识体系）
                            item_id: crate::items::item_uuid(&m.item_id).to_string(),
                            item_name: item_config
                                .as_ref()
                                .map(|c| c.name.clone())
                                .unwrap_or_else(|| m.item_id.clone()),
                            quantity: m.quantity,
                        }
                    })
                    .collect();
            Some(cyber_jianghu_protocol::types::entities::RecipeDetail {
                recipe_id: crate::game_data::registry::RecipeRegistry::uuid(recipe_id).to_string(),
                name: recipe.name,
                description: recipe.description,
                materials,
                result_item: crate::items::item_uuid(&recipe.result_item).to_string(),
                result_item_name: result_item_config
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| recipe.result_item.clone()),
                result_quantity: recipe.result_quantity,
                stamina_cost: recipe.stamina_cost,
            })
        })
        .collect()
}
