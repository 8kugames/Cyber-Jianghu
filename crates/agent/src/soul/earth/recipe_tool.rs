// ============================================================================
// list_known_recipes / view_recipe_detail 工具定义与执行
// ============================================================================

use crate::component::llm::tool_types::ToolDefinition;
use cyber_jianghu_protocol::types::entities::RecipeDetail;

/// list_known_recipes tool 定义
pub fn list_known_recipes_definition() -> ToolDefinition {
    ToolDefinition::new(
        "list_known_recipes",
        "列出你已知的所有配方（配方ID和名称）。使用 view_recipe_detail 查看某个配方的详细材料要求和产出。",
        Some(serde_json::json!({
            "type": "object",
            "properties": {}
        })),
    )
}

/// view_recipe_detail tool 定义
pub fn view_recipe_detail_definition() -> ToolDefinition {
    ToolDefinition::new(
        "view_recipe_detail",
        "查看某个配方的详细信息：所需材料、产出物品、体力消耗。用于决定制造什么以及准备什么材料。",
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "recipe_id": {
                    "type": "string",
                    "description": "配方ID（从 list_known_recipes 获取）"
                }
            },
            "required": ["recipe_id"]
        })),
    )
}

/// 执行 list_known_recipes
pub fn execute_list_known_recipes(recipe_details: &[RecipeDetail]) -> serde_json::Value {
    if recipe_details.is_empty() {
        return serde_json::json!({
            "success": true,
            "message": "你目前不知道任何配方。可以通过观察他人制造来学习，或请燧人氏传授。"
        });
    }

    let recipes: Vec<serde_json::Value> = recipe_details
        .iter()
        .map(|r| {
            serde_json::json!({
                "recipe_id": r.recipe_id,
                "name": r.name,
                "result": format!("{}x{}", r.result_item_name, r.result_quantity),
            })
        })
        .collect();

    serde_json::json!({
        "success": true,
        "count": recipes.len(),
        "recipes": recipes,
        "hint": "使用 view_recipe_detail 查看某个配方的详细材料要求"
    })
}

/// 执行 view_recipe_detail
pub fn execute_view_recipe_detail(
    recipe_id: &str,
    recipe_details: &[RecipeDetail],
) -> serde_json::Value {
    let detail = recipe_details.iter().find(|r| {
        r.recipe_id == recipe_id
            || r.recipe_id.ends_with(&format!("/{}", recipe_id))
            || r.name == recipe_id
    });

    match detail {
        Some(d) => {
            let materials: Vec<serde_json::Value> = d
                .materials
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "item": m.item_name,
                        "item_id": m.item_id,
                        "quantity": m.quantity,
                    })
                })
                .collect();

            serde_json::json!({
                "success": true,
                "recipe_id": d.recipe_id,
                "name": d.name,
                "description": d.description,
                "materials": materials,
                "result_item": d.result_item_name,
                "result_item_id": d.result_item,
                "result_quantity": d.result_quantity,
                "stamina_cost": d.stamina_cost,
            })
        }
        None => serde_json::json!({
            "success": false,
            "message": format!("你不知道配方「{}」。使用 list_known_recipes 查看已知配方。", recipe_id)
        }),
    }
}
