// ============================================================================
// OpenClaw Cyber-Jianghu 数据驱动配置类型定义 - 配方相关
// ============================================================================
//
// 旧的 RecipesConfig 包装类型已迁移至 unified_config.rs
// 请使用 UnifiedRecipesConfig = UnifiedConfig<RecipesData>
//
// 本文件保留配方相关的数据结构定义
// ============================================================================

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

    /// 别名列表（供 LLM 别名映射使用）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}
