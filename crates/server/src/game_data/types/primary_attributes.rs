// ============================================================================
// OpenClaw Cyber-Jianghu 主属性（先天属性）配置类型定义
// ============================================================================
//
// 本模块包含主属性配置相关类型 (primary_attributes.json)
// 采用数据驱动架构，使用字符串常量而非枚举
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 主属性配置
///
/// 预留：主属性配置系统待集成
#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct PrimaryAttributesConfig {
    /// 配置版本号
    pub version: String,

    /// 配置描述
    #[serde(default = "default_primary_description")]
    pub description: String,

    /// 主属性元数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<PrimaryAttributesMetadata>,

    /// 主属性定义映射
    pub attributes: HashMap<String, PrimaryAttributeDefinition>,
}

#[allow(dead_code)]
fn default_primary_description() -> String {
    "Primary attributes configuration".to_string()
}

/// 主属性元数据
#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct PrimaryAttributesMetadata {
    /// 总属性数量
    pub total_attributes: usize,

    /// 可成长属性数量
    pub growable_attributes: usize,

    /// 静态属性数量
    pub static_attributes: usize,

    /// 每日随机属性数量
    pub daily_random_attributes: usize,
}

/// 主属性定义
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PrimaryAttributeDefinition {
    /// 属性名称（英文key）
    pub name: String,

    /// 显示名称
    pub display_name: String,

    /// 描述
    pub description: String,

    /// 属性类型
    #[serde(rename = "type")]
    pub type_name: PrimaryAttributeType,

    /// 出生范围（用于随机生成初始值）
    pub birth_range: Option<(i32, i32)>,

    /// 初始值（固定值）
    pub initial_value: Option<i32>,

    /// 成长率
    pub growth_rate: Option<f64>,

    /// 影响的属性列表（用于派生属性计算）
    pub affects: Vec<String>,
}

/// 主属性类型（字符串常量，数据驱动）
///
/// 使用字符串而非枚举，添加新类型无需修改代码
/// 常见值:
/// - "growable": 可成长属性
/// - "static": 静态属性
/// - "daily_random": 每日随机属性
pub type PrimaryAttributeType = String;

/// 主属性类型常量
pub const PRIMARY_ATTR_GROWABLE: &str = "growable";
pub const PRIMARY_ATTR_STATIC: &str = "static";
pub const PRIMARY_ATTR_DAILY_RANDOM: &str = "daily_random";

/// 检查主属性类型是否为可成长属性
#[allow(dead_code)]
pub fn is_growable_attr(attr_type: &PrimaryAttributeType) -> bool {
    attr_type == PRIMARY_ATTR_GROWABLE
}

/// 检查主属性类型是否为静态属性
#[allow(dead_code)]
pub fn is_static_attr(attr_type: &PrimaryAttributeType) -> bool {
    attr_type == PRIMARY_ATTR_STATIC
}

/// 检查主属性类型是否为每日随机属性
#[allow(dead_code)]
pub fn is_daily_random_attr(attr_type: &PrimaryAttributeType) -> bool {
    attr_type == PRIMARY_ATTR_DAILY_RANDOM
}
