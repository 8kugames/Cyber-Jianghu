// ============================================================================
// 展示名统一格式化
// ============================================================================
//
// 全服唯一的"可读名 + 可还原标识"展示格式定义：
// - 角色：姓名[短 uuid 前 8 位]
// - 物品：名称[短 uuid 前 8 位]（uuid 由 item_id 经 UUID v5 确定性派生）
//
// 任何展示面（动作结果消息、WorldEvent、chronicle、dashboard API）都应
// 从本模块取格式化结果，禁止各处自行拼接。

/// 角色展示名：姓名[短 uuid 前 8 位]。
///
/// 既可读（姓名），又可还原（短 uuid 可对应 dashboard 查询）。
pub fn display_agent_name(name: &str, agent_id: uuid::Uuid) -> String {
    format!("{}[{}]", name, &agent_id.to_string()[..8])
}

/// 物品展示名：名称[短 uuid 前 8 位]。
///
/// 实现在 [`crate::items`]（需访问物品定义缓存），此处转发作为统一入口。
/// 物品未注册（查不到名称）时退化为裸 item_id。
pub use crate::items::display_item_name;

/// 物品稳定 uuid（UUID v5，从 item_id 确定性派生）。
pub use crate::items::item_uuid;

/// 配方展示名：名称[短 uuid 前 8 位]。
///
/// 实现在 [`crate::game_data::registry::RecipeRegistry`]，此处包装作为统一入口。
/// 配方未注册（查不到名称）时退化为裸 recipe_id。
pub fn display_recipe_name(recipe_id: &str) -> String {
    crate::game_data::registry::RecipeRegistry::display_name(recipe_id)
}

/// 配方稳定 uuid（UUID v5，从 recipe_id 确定性派生）。
pub fn recipe_uuid(recipe_id: &str) -> uuid::Uuid {
    crate::game_data::registry::RecipeRegistry::uuid(recipe_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_agent_name_format() {
        let id = uuid::Uuid::new_v4();
        let s = id.to_string();
        assert_eq!(
            display_agent_name("沈暮云", id),
            format!("沈暮云[{}]", &s[..8])
        );
    }
}
