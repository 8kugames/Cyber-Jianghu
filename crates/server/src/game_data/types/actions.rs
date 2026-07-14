// ============================================================================
// OpenClaw Cyber-Jianghu 数据驱动配置类型定义 - 动作相关
// ============================================================================
//
// 本模块定义动作相关的数据结构，采用数据驱动和 COI 架构
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use cyber_jianghu_protocol::types::governance::{AtomicKind, ProtocolKind, TargetArity};
use cyber_jianghu_protocol::types::OocRisk;

// ============================================================================
// 动作配置条目
// ============================================================================

/// 动作配置条目
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActionConfigEntry {
    /// 动作名称（简短标识）
    #[serde(default)]
    pub name: String,

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

    /// 逃跑默认成功率（公式不存在或公式计算失败时使用）
    #[serde(default = "default_flee_success_rate")]
    pub default_flee_success_rate: f64,

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

    /// OOC 风险等级
    /// 用于天魂分级审核：High → 强制 LLM, Medium → 抽审, Low → 跳过 LLM
    #[serde(default)]
    pub ooc_risk: OocRisk,

    /// 显示名（用于 chronicle_generator 等展示场景，未配置时回退到 `name`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,

    /// 传输语义（决定动作执行后如何向其他 Agent 传播，详见 `Transmission` 枚举）
    #[serde(default)]
    pub transmission: Transmission,

    /// 预验证器种类（详见 `ValidatorKind`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validator_kind: Option<ValidatorKind>,

    /// 编年史高光种类（详见 `HighlightKind`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight_kind: Option<HighlightKind>,

    /// 原子行为类型（v6 §4.5 补字段，序列化时 lowercase）
    #[serde(default)]
    pub atomic_kind: AtomicKind,

    /// 执行者数量（v6 §4.5 补字段）
    #[serde(default = "default_actor_arity")]
    pub actor_arity: u8,

    /// 目标数量范围（v6 §4.5 补字段，序列化时 snake_case）
    #[serde(default)]
    pub target_arity: TargetArity,

    /// 持续 tick 数（v6 §4.5 补字段）
    #[serde(default)]
    pub tick_span: u8,

    /// 阶段数（v6 §4.5 补字段）
    #[serde(default = "default_phase_count")]
    pub phase_count: u8,

    /// 协议编排类型（v6 §4.5 补字段，序列化时 snake_case）
    #[serde(default)]
    pub protocol_kind: ProtocolKind,
}

fn default_actor_arity() -> u8 {
    1
}

fn default_phase_count() -> u8 {
    1
}

fn default_flee_success_rate() -> f64 {
    0.5
}

// ============================================================================
// 动作传输语义（数据驱动扩展）
// ============================================================================

/// 动作传输语义
///
/// 决定动作执行后的传播行为：
/// - `Broadcast`: 公共频道广播给同 Location 的所有 Agent（默认）
/// - `Session`: 定向 + 服务器维护 Dialogue Session
/// - `Silent`: 触发方动作，仅修改状态不广播
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transmission {
    #[default]
    Broadcast,
    Session,
    Silent,
}

// ============================================================================
// 预验证器种类（数据驱动扩展 — Phase 5 收尾）
// ============================================================================

/// 预验证器种类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidatorKind {
    RecipeKnowledge,
    TeachRecipe,
}

// ============================================================================
// 编年史高光种类（数据驱动扩展 — Phase 5 收尾）
// ============================================================================

/// 编年史高光种类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HighlightKind {
    Dialogue,
    Combat,
    Social,
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

    /// 是否需要目标与发起者在同一地点
    pub requires_target_colocated: Option<bool>,

    /// 必需的数据字段
    pub required_fields: Vec<String>,

    /// 可选的数据字段
    #[serde(default)]
    pub optional_fields: Vec<String>,

    /// 字段验证规则
    pub field_validations: Vec<FieldValidation>,
}

/// 字段验证规则
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FieldValidation {
    /// 字段名称
    pub field: String,

    /// 验证类型
    pub validation_type: ValidationType,

    /// 验证参数
    #[serde(flatten)]
    pub params: std::collections::HashMap<String, serde_json::Value>,
}

/// 字段校验类型（6 值闭集，dispatch 在 actions/validator.rs:192 的 match）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationType {
    NotEmpty,
    MinValue,
    MaxValue,
    MinLength,
    MaxLength,
    /// 校验字段值（如 item_id）必须在物品注册表（items.yaml）中存在
    ItemExists,
}

impl FieldValidation {

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
    /// 需求类型
    pub requirement_type: cyber_jianghu_protocol::RequirementType,

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
    /// 效果类型
    pub effect_type: cyber_jianghu_protocol::EffectType,

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

impl ActionRequirement {
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

impl ActionEffect {
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
