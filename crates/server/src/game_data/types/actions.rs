// ============================================================================
// OpenClaw Cyber-Jianghu 数据驱动配置类型定义 - 动作相关
// ============================================================================
//
// 本模块定义动作相关的数据结构，采用数据驱动和 COI 架构
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// 动作配置条目
// ============================================================================

/// 动作配置条目
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActionConfigEntry {
    /// 动作名称（简短标识）
    #[serde(default)]
    pub name: String,

    /// 动作类型别名（中文变体 + 常见英文错误）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,

    /// 动作描述（详细说明行为和效果，避免 LLM 理解歧义）
    #[serde(default)]
    pub description: String,

    /// 动作分类（survival/combat/martial/social/economic）
    #[serde(default)]
    pub category: String,

    /// 动作标签（用于分类和逻辑判断，如 [survival]）
    #[serde(default)]
    pub tags: Vec<String>,

    /// 基础伤害（attack）
    pub base_damage: Option<i32>,

    /// 伤害公式（attack）
    pub damage_formula: Option<String>,

    /// 武器加成值（attack）
    pub weapon_bonus: Option<i32>,

    /// 武器加成倍率（attack）
    pub weapon_bonus_multiplier: Option<f32>,

    /// 成功率（steal）
    pub success_rate: Option<f32>,

    /// 逃跑成功率公式（flee）
    pub flee_success_formula: Option<String>,

    /// 最大内容长度（speak）
    pub max_content_length: Option<i32>,

    /// 体力消耗
    pub stamina_cost: Option<i32>,

    /// 验证规则（数据驱动）
    #[serde(default)]
    pub validation: Option<ActionValidation>,

    /// 通用需求列表
    #[serde(default)]
    pub requirements: Vec<ActionRequirement>,

    /// 通用效果列表
    #[serde(default)]
    pub effects: Vec<ActionEffect>,

    /// OOC 风险等级（"low" | "medium" | "high"）
    /// 用于天魂分级审核：high → 强制 LLM, medium → 抽审, low → 跳过 LLM
    #[serde(default = "default_ooc_risk")]
    pub ooc_risk: String,
}

fn default_ooc_risk() -> String {
    "low".to_string()
}

// ============================================================================
// 动作验证规则（数据驱动）
// ============================================================================

/// 动作验证规则（数据驱动）
///
/// 定义动作执行前需要验证的规则
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct ActionValidation {
    /// 是否需要目标（Agent）
    pub requires_target: Option<bool>,

    /// 是否需要目标存活
    pub requires_target_alive: Option<bool>,

    /// 必需的数据字段
    pub required_fields: Vec<String>,

    /// 字段验证规则
    pub field_validations: Vec<FieldValidation>,

    /// 字段别名映射 { canonical_field_name: [aliases...] }
    /// 用于 LLM 输出的中文/变体字段名 → 英文 canonical 字段名翻译
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub field_aliases: HashMap<String, Vec<String>>,
}

/// 字段验证规则
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FieldValidation {
    /// 字段名称
    pub field: String,

    /// 验证类型
    pub validation_type: String,

    /// 验证参数
    #[serde(flatten)]
    pub params: std::collections::HashMap<String, serde_json::Value>,
}

impl FieldValidation {
    pub const TYPE_NOT_EMPTY: &str = "not_empty";
    pub const TYPE_MIN_VALUE: &str = "min_value";
    pub const TYPE_MAX_VALUE: &str = "max_value";
    pub const TYPE_MIN_LENGTH: &str = "min_length";
    pub const TYPE_MAX_LENGTH: &str = "max_length";

    /// 获取 i32 参数
    pub fn get_i32(&self, key: &str) -> Option<i32> {
        self.params.get(key).and_then(|v| {
            if v.is_i64() {
                v.as_i64().map(|v| v as i32)
            } else if v.is_f64() {
                v.as_f64().map(|v| v as i32)
            } else {
                None
            }
        })
    }
}

// ============================================================================
// 动作需求（数据驱动）
// ============================================================================

/// 动作需求（数据驱动结构）
///
/// 使用 `type` 字段区分不同需求类型，而非枚举变体
/// 这样添加新需求类型无需修改核心数据结构
///
/// 示例：
/// ```json
/// {
///   "type": "attribute",
///   "target": "target",
///   "attribute": "stamina",
///   "min": 10,
///   "consume": true
/// }
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ActionRequirement {
    /// 需求类型（如 "attribute", "item", "location" 等）
    /// 添加新类型无需修改此结构，只需在配置和处理器中支持
    pub requirement_type: String,

    /// 目标对象（如 "target", "self"）
    #[serde(default)]
    pub target: String,

    /// 额外参数（以键值对形式存储，支持任意扩展）
    /// 常见参数：
    /// - attribute: 属性名称（attribute 类型）
    /// - min: 最小值（attribute 类型）
    /// - item_id: 物品ID（item 类型）
    /// - quantity: 数量（item 类型）
    /// - consume: 是否消耗（boolean）
    #[serde(flatten)]
    pub params: HashMap<String, serde_json::Value>,
}

// ============================================================================
// 动作效果（数据驱动）
// ============================================================================

/// 动作效果（数据驱动结构）
///
/// 使用 `type` 字段区分不同效果类型，而非枚举变体
/// 这样添加新效果类型无需修改核心数据结构
///
/// 示例：
/// ```json
/// {
///   "type": "attribute_change",
///   "target": "target",
///   "attribute": "hp",
///   "operation": "add",
///   "value": -10
/// }
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ActionEffect {
    /// 效果类型（如 "attribute_change", "add_item", "remove_item" 等）
    /// 添加新类型无需修改此结构，只需在配置和处理器中支持
    pub effect_type: String,

    /// 目标对象（如 "target", "self"）
    #[serde(default)]
    pub target: String,

    /// 额外参数（以键值对形式存储，支持任意扩展）
    /// 常见参数：
    /// - attribute: 属性名称（attribute_change 类型）
    /// - operation: 操作类型（"add", "set", "multiply"）
    /// - value: 效果值（可以是数字或字符串）
    /// - item_id: 物品ID（add_item 类型）
    /// - quantity: 数量（add_item 类型）
    #[serde(flatten)]
    pub params: HashMap<String, serde_json::Value>,
}

// ============================================================================
// 辅助常量（数据驱动扩展预留）
// ============================================================================

/// 需求类型常量
#[allow(dead_code)]
impl ActionRequirement {
    pub const REQUIREMENT_TYPE_ATTRIBUTE: &str = "attribute";
    pub const REQUIREMENT_TYPE_ITEM: &str = "item";
    pub const REQUIREMENT_TYPE_LOCATION: &str = "location";
    pub const REQUIREMENT_TYPE_SKILL: &str = "skill";

    /// 获取字符串参数
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(|v| v.as_str())
    }

    /// 获取 i32 参数
    pub fn get_i32(&self, key: &str) -> Option<i32> {
        self.params.get(key).and_then(|v| {
            if v.is_i64() {
                v.as_i64().map(|v| v as i32)
            } else if v.is_f64() {
                v.as_f64().map(|v| v as i32)
            } else {
                None
            }
        })
    }

    /// 获取 bool 参数
    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.params
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }

    /// 获取属性消耗值（cost）
    /// 返回正值表示扣减量
    pub fn get_cost(&self) -> Option<i32> {
        self.get_i32("cost").filter(|&v| v > 0)
    }

    /// 获取属性名称
    pub fn get_attribute(&self) -> Option<&str> {
        self.get_str("attribute")
    }

    /// 获取属性最小值要求
    pub fn get_min(&self) -> Option<i32> {
        self.get_i32("min")
    }
}

/// 效果类型常量
#[allow(dead_code)]
impl ActionEffect {
    pub const EFFECT_TYPE_ATTRIBUTE_CHANGE: &str = "attribute_change";
    pub const EFFECT_TYPE_ATTRIBUTE_MAX_CHANGE: &str = "attribute_max_change";
    pub const EFFECT_TYPE_ADD_ITEM: &str = "add_item";
    pub const EFFECT_TYPE_REMOVE_ITEM: &str = "remove_item";
    pub const EFFECT_TYPE_TELEPORT: &str = "teleport";

    /// 获取字符串参数
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(|v| v.as_str())
    }

    /// 获取 i32 参数
    pub fn get_i32(&self, key: &str) -> Option<i32> {
        self.params.get(key).and_then(|v| {
            if v.is_i64() {
                v.as_i64().map(|v| v as i32)
            } else if v.is_f64() {
                v.as_f64().map(|v| v as i32)
            } else {
                None
            }
        })
    }

    /// 获取 f64 参数
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.params.get(key).and_then(|v| v.as_f64())
    }
}
