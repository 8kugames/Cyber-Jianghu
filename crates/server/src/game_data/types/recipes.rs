// 配方相关数据结构定义。
// 请使用 UnifiedRecipesConfig = UnifiedConfig<RecipesData>

use serde::{Deserialize, Serialize};

// ============================================================================
// 配方定义
// ============================================================================

/// 配方材料要求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeMaterial {
    pub item_id: String,
    pub quantity: i32,
}

/// 单个配方定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeDefinition {
    pub name: String,
    pub description: String,
    pub result_item: String,
    pub result_quantity: i32,
    pub materials: Vec<RecipeMaterial>,
    pub tools: Vec<String>,
    pub stamina_cost: i32,
}
