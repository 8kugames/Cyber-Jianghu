// ============================================================================
// OpenClaw Cyber-Jianghu 状态值配置类型定义
// ============================================================================
//
// 本模块包含状态值和派生属性配置相关类型
// 采用数据驱动架构，使用 serde_json::Value 作为统一的值类型
// ============================================================================

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

use super::validation::DeathCondition;

/// 属性配置（用于状态值和派生属性）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AttributesConfig {
    /// 配置版本号
    pub version: String,

    /// 配置描述
    #[serde(default = "default_attributes_description")]
    pub description: String,

    /// 属性定义映射
    pub attributes: HashMap<String, AttributeDefinition>,
}

fn default_attributes_description() -> String {
    "Attributes configuration".to_string()
}

/// 属性类型（字符串常量，数据驱动）
///
/// 使用字符串而非枚举，添加新类型无需修改代码
/// 常见值: "integer", "float", "string", "boolean"
pub type AttributeType = String;

/// 属性定义（用于状态值和派生属性）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AttributeDefinition {
    /// 属性名称
    pub name: String,

    /// 显示名称
    pub display_name: String,

    /// 描述
    pub description: String,

    /// 属性类型（字符串，数据驱动）
    #[serde(rename = "type")]
    pub type_name: AttributeType,

    /// 默认值（JsonValue 支持任意类型）
    #[serde(default)]
    pub default_value: Option<JsonValue>,

    /// 最小值
    #[serde(default)]
    pub min_value: Option<JsonValue>,

    /// 最大值
    #[serde(default)]
    pub max_value: Option<JsonValue>,

    /// 每tick衰减值
    #[serde(default)]
    pub decay_per_tick: Option<JsonValue>,

    /// 死亡条件
    #[serde(default)]
    pub death_condition: Option<DeathCondition>,

    /// 计算公式
    #[serde(default)]
    pub formula: Option<String>,

    /// 恢复公式
    #[serde(default)]
    pub recovery_formula: Option<String>,

    /// 依赖的主属性列表
    #[serde(default)]
    pub primary_attribute_deps: Option<Vec<String>>,

    /// 备注（用于兼容旧代码）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl AttributeDefinition {
    /// 获取 default_value 的整数值
    pub fn default_value_as_i32(&self) -> Option<i32> {
        self.default_value.as_ref().and_then(|v| {
            v.as_i64().map(|i| i as i32)
        })
    }

    /// 获取 min_value 的整数值
    pub fn min_value_as_i32(&self) -> Option<i32> {
        self.min_value.as_ref().and_then(|v| {
            v.as_i64().map(|i| i as i32)
        })
    }

    /// 获取 max_value 的整数值
    pub fn max_value_as_i32(&self) -> Option<i32> {
        self.max_value.as_ref().and_then(|v| {
            v.as_i64().map(|i| i as i32)
        })
    }

    /// 获取 decay_per_tick 的整数值
    pub fn decay_per_tick_as_i32(&self) -> Option<i32> {
        self.decay_per_tick.as_ref().and_then(|v| {
            v.as_i64().map(|i| i as i32)
        })
    }
}
