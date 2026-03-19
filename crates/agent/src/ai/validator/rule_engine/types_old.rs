// ============================================================================
// 规则引擎类型定义
// ============================================================================
//
// 提供规则引擎验证所需的类型定义

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::ai::llm::LlmClient;
use crate::models::Intent;
use cyber_jianghu_protocol::ActionType;

use super::types::{PersonaInfo, RejectionType, ValidationRequest, ValidationResult};

// ============================================================================
// 规则定义
// ============================================================================

/// 规则类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuleType {
    /// 动作冷却规则
    ActionCooldown,
    /// 资源约束规则
    ResourceConstraint,
    /// 状态限制规则
    StateRestriction,
    /// 特质一致性规则
    TraitConsistency,
    /// 数值范围规则
    ValueRange,
    /// 自定义规则
    Custom,
}

/// 规则条件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleCondition {
    /// 等于
    Equals(String, serde_json::Value),
    /// 不等于
    NotEquals(String, serde_json::Value),
    /// 大于
    GreaterThan(String, f64),
    /// 小于
    LessThan(String, f64),
    /// 包含
    Contains(String, String),
    /// 不包含
    NotContains(String, String),
    /// 且（AND）
    And(Vec<RuleCondition>),
    /// 或（OR）
    Or(Vec<RuleCondition>),
    /// 非（NOT）
    Not(Box<RuleCondition>),
}

/// 规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// 规则 ID
    pub id: String,
    /// 规则名称
    pub name: String,
    /// 规则类型
    pub rule_type: RuleType,
    /// 规则条件
    pub condition: RuleCondition,
    /// 错误消息
    pub error_message: String,
    /// 是否启用
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Rule {
    /// 创建新的规则
    pub fn new(
        id: String,
        name: String,
        rule_type: RuleType,
        condition: RuleCondition,
        error_message: String,
    ) -> Self {
        Self {
            id,
            name,
            rule_type,
            condition,
            error_message,
            enabled: true,
        }
    }

    /// 创建禁用的规则
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

// ============================================================================
// 规则验证上下文
// ============================================================================

/// 规则验证上下文
///
/// 提供规则执行时需要的信息
#[derive(Debug, Clone)]
pub struct RuleValidationContext {
    /// 意图
    pub intent: Intent,
    /// 人设信息
    pub persona_info: PersonaInfo,
    /// 世界上下文（自然语言描述)
    pub world_context: String,
    /// 当前 Tick ID
    pub tick_id: i64,
    /// 历史意图(用于冷却检查)
    pub history_intents: Vec<Intent>,
    /// 额外的属性数据(用于规则检查)
    pub attributes: HashMap<String, serde_json::Value>,
}

impl RuleValidationContext {
    /// 从 ValidationRequest 创建上下文
    pub fn from_request(
        request: ValidationRequest,
        history_intents: Vec<Intent>,
        attributes: HashMap<String, serde_json::Value>,
    ) -> Self {
        let tick_id = request.intent.tick_id;
        Self {
            intent: request.intent,
            persona_info: request.persona,
            world_context: request.world_context,
            tick_id,
            history_intents,
            attributes,
        }
    }

    /// 获取意图的动作类型
    pub fn action_type(&self) -> &ActionType {
        &self.intent.action_type
    }

    /// 从属性数据中获取值
    pub fn get_attribute(&self, key: &str) -> Option<&serde_json::Value> {
        self.attributes.get(key)
    }
}

// ============================================================================
// 规则验证结果
// ============================================================================

/// 单个规则的验证结果
#[derive(Debug, Clone)]
pub struct RuleValidationResult {
    /// 规则 ID
    pub rule_id: String,
    /// 是否通过
    pub passed: bool,
    /// 错误消息(如果未通过)
    pub error_message: Option<String>,
}

impl RuleValidationResult {
    /// 创建通过的结果
    pub fn passed(rule_id: String) -> Self {
        Self {
            rule_id,
            passed: true,
            error_message: None,
        }
    }

    /// 创建失败的结果
    pub fn failed(rule_id: String, error_message: String) -> Self {
        Self {
            rule_id,
            passed: false,
            error_message: Some(error_message),
        }
    }
}

// ============================================================================
// 规则引擎配置
// ============================================================================

/// 规则引擎配置
#[derive(Debug, Clone)]
pub struct RuleEngineConfig {
    /// 是否启用特质一致性检查
    pub enable_trait_consistency: bool,
    /// 是否启用动作冷却检查
    pub enable_action_cooldown: bool,
    /// 默认冷却 Tick 数
    pub default_cooldown_ticks: i64,
    /// 是否启用资源约束检查
    pub enable_resource_constraints: bool,
    /// 连续失败触发深度验证的阈值
    /// 当连续 N 次验证失败后，触发 LLM 深度验证
    pub consecutive_failures_for_deep_verify: usize,
    /// 是否启用连续失败后的 LLM 深度验证
    pub enable_deep_verify_on_repeated_fail: bool,
}

impl Default for RuleEngineConfig {
    fn default() -> Self {
        Self {
            enable_trait_consistency: true,
            enable_action_cooldown: true,
            default_cooldown_ticks: 5,
            enable_resource_constraints: true,
            consecutive_failures_for_deep_verify: 3,
            enable_deep_verify_on_repeated_fail: true,
        }
    }
}
