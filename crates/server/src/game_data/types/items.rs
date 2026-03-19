// ============================================================================
// OpenClaw Cyber-Jianghu 数据驱动配置类型定义 - 物品相关
// ============================================================================
//
// 旧的 ItemsConfig 包装类型已迁移至 unified_config.rs
// 请使用 UnifiedItemsConfig = UnifiedConfig<ItemsData>
//
// 本文件保留物品相关的数据结构定义
// ============================================================================

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

// ============================================================================
// 物品配置条目
// ============================================================================

/// 物品配置条目
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ItemConfigEntry {
    /// 物品唯一ID
    pub item_id: String,

    /// 物品名称
    pub name: String,

    /// 物品类型
    pub item_type: String,

    /// 效果列表
    #[serde(default)]
    pub effects: Vec<ItemEffect>,

    /// 物品描述
    pub description: String,

    /// 可堆叠数量
    pub stack_size: i32,

    /// 最大耐久度（默认 -1 表示无限）
    #[serde(default = "default_max_durability")]
    pub max_durability: i32,

    /// 自然衰减速率（每Tick减少的耐久度，默认 0）
    #[serde(default = "default_decay_rate")]
    pub decay_rate: i32,
}

fn default_max_durability() -> i32 {
    -1
}

fn default_decay_rate() -> i32 {
    0
}

// ============================================================================
// 物品效果（数据驱动）
// ============================================================================

/// 物品效果（数据驱动结构）
///
/// 使用 JsonValue 存储效果值，支持任意类型
/// 不需要枚举来限制值的类型
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ItemEffect {
    /// 效果描述
    #[serde(default)]
    pub description: String,

    /// 目标属性
    #[serde(default)]
    pub attribute: String,

    /// 操作类型（如 "add", "set", "multiply"）
    #[serde(default)]
    pub operation: String,

    /// 效果值（可以是数字、字符串、布尔值等）
    #[serde(default)]
    pub value: JsonValue,
}

impl ItemEffect {
    /// 获取整数值
    pub fn value_as_i32(&self) -> Option<i32> {
        match &self.value {
            JsonValue::Number(n) => n.as_i64().map(|v| v as i32),
            _ => None,
        }
    }

    /// 获取浮点数值
    #[allow(dead_code)]
    pub fn value_as_f64(&self) -> Option<f64> {
        match &self.value {
            JsonValue::Number(n) => n.as_f64(),
            _ => None,
        }
    }

    /// 获取字符串值
    #[allow(dead_code)]
    pub fn value_as_str(&self) -> Option<&str> {
        match &self.value {
            JsonValue::String(s) => Some(s),
            _ => None,
        }
    }
}
