// ============================================================================
// OpenClaw Cyber-Jianghu 统一属性配置类型定义
// ============================================================================
//
// 本模块包含统一属性配置相关类型 (attributes.json)
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::primary_attributes::PrimaryAttributeDefinition;
use super::validation::DeathCondition;

/// 属性分类（数据部分）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AttributeCategories {
    /// 主属性（先天属性)
    pub primary: PrimaryAttributesCategory,

    /// 状态值（生理/精神状态)
    pub status: StatusAttributesCategory,

    /// 派生属性（基于先天属性实时计算)
    pub derived: DerivedAttributesCategory,
}

// 在这里定义统一属性配置类型，避免循环依赖
/// 统一属性配置（使用统一格式）
pub type UnifiedAttributesConfig = super::unified_config::UnifiedConfig<AttributeCategories>;

/// 主属性分类
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PrimaryAttributesCategory {
    /// 分类描述
    pub description: String,

    /// 主属性定义映射
    pub attributes: HashMap<String, PrimaryAttributeDefinition>,
}

/// 状态值分类
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatusAttributesCategory {
    /// 分类描述
    pub description: String,

    /// 状态值定义映射
    pub attributes: HashMap<String, StatusAttributeDefinition>,
}

/// 派生属性分类
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DerivedAttributesCategory {
    /// 分类描述
    pub description: String,

    /// 派生属性定义映射
    pub attributes: HashMap<String, DerivedAttributeDefinition>,
}

/// 派生属性定义（统一格式）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DerivedAttributeDefinition {
    /// 属性名称（英文key）
    pub name: String,

    /// 显示名称
    pub display_name: String,

    /// 描述
    pub description: String,

    /// 属性类型（固定为 derived)
    #[serde(rename = "type")]
    pub type_name: String,

    /// 计算公式
    pub formula: Option<String>,

    /// 默认值
    pub default_value: Option<f64>,

    /// 最小值
    pub min_value: Option<f64>,

    /// 最大值
    pub max_value: Option<f64>,

    /// 依赖的主属性列表
    pub primary_attribute_deps: Option<Vec<String>>,
}

/// 状态值定义（统一格式）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatusAttributeDefinition {
    /// 属性名称（英文key）
    pub name: String,

    /// 显示名称
    pub display_name: String,

    /// 描述
    pub description: String,

    /// 属性类型(固定为 status)
    #[serde(rename = "type")]
    pub type_name: String,

    /// 计算公式
    pub formula: Option<String>,

    /// 默认值
    pub default_value: Option<f64>,

    /// 最小值
    pub min_value: Option<f64>,

    /// 最大值公式
    pub max_value_formula: Option<String>,

    /// 每tick衰减值
    pub decay_per_tick: Option<f64>,

    /// 恢复公式
    pub recovery_formula: Option<String>,

    /// 死亡条件
    pub death_condition: Option<DeathCondition>,

    /// 依赖的主属性列表
    pub primary_attribute_deps: Option<Vec<String>>,
}
