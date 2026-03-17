use crate::game_data::registry_or_panic;
use crate::game_data::types::RecipeDefinition;

/// 配方注册表
///
/// 提供对配方配置的安全访问
pub struct RecipeRegistry;

impl RecipeRegistry {
    /// 获取配方定义
    pub fn get(recipe_id: &str) -> Option<RecipeDefinition> {
        let registry = registry_or_panic();
        registry.get().recipes.data.get(recipe_id).cloned()
    }
}