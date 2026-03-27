//! 规则类型定义
//!
//! 定义规则引擎使用的所有数据类型。

use crate::ai::validator::types::{PersonaInfo, ValidationRequest};
use crate::models::Intent;
use cyber_jianghu_protocol::ActionType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// 规则验证上下文
///
/// 提供规则执行时需要的信息
#[derive(Debug, Clone)]
pub struct RuleValidationContext {
    /// 意图
    pub intent: Intent,
    /// 人设信息
    pub persona_info: PersonaInfo,
    /// 世界上下文（自然语言描述）
    pub world_context: String,
    /// 当前 Tick ID
    pub tick_id: i64,
    /// 历史意图（用于冷却检查）
    pub history_intents: Vec<Intent>,
    /// 额外的属性数据（用于规则检查）
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

/// 单个规则的验证结果
#[derive(Debug, Clone)]
pub struct RuleValidationResult {
    /// 规则 ID
    pub rule_id: String,
    /// 是否通过
    pub passed: bool,
    /// 错误消息（如果未通过）
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_type_variants() {
        // 验证所有 RuleType 变体都可以创建
        let _ = RuleType::ActionCooldown;
        let _ = RuleType::ResourceConstraint;
        let _ = RuleType::StateRestriction;
        let _ = RuleType::TraitConsistency;
        let _ = RuleType::ValueRange;
        let _ = RuleType::Custom;
    }

    #[test]
    fn test_rule_creation() {
        let rule = Rule::new(
            "test_rule".to_string(),
            "Test Rule".to_string(),
            RuleType::ActionCooldown,
            RuleCondition::GreaterThan("cooldown".to_string(), 0.0),
            "Test error message".to_string(),
        );

        assert_eq!(rule.id, "test_rule");
        assert_eq!(rule.name, "Test Rule");
        assert!(rule.enabled);
    }

    #[test]
    fn test_rule_disabled() {
        let rule = Rule::new(
            "test_rule".to_string(),
            "Test Rule".to_string(),
            RuleType::ActionCooldown,
            RuleCondition::GreaterThan("cooldown".to_string(), 0.0),
            "Test error message".to_string(),
        )
        .disabled();

        assert!(!rule.enabled);
    }

    #[test]
    fn test_rule_validation_result_passed() {
        let result = RuleValidationResult::passed("rule_1".to_string());
        assert!(result.passed);
        assert!(result.error_message.is_none());
        assert_eq!(result.rule_id, "rule_1");
    }

    #[test]
    fn test_rule_validation_result_failed() {
        let result = RuleValidationResult::failed("rule_1".to_string(), "Test error".to_string());
        assert!(!result.passed);
        assert_eq!(result.error_message, Some("Test error".to_string()));
        assert_eq!(result.rule_id, "rule_1");
    }

    #[test]
    fn test_rule_engine_config_default() {
        let config = RuleEngineConfig::default();
        assert!(config.enable_trait_consistency);
        assert!(config.enable_action_cooldown);
        assert!(config.enable_resource_constraints);
        assert_eq!(config.default_cooldown_ticks, 5);
        assert_eq!(config.consecutive_failures_for_deep_verify, 3);
        assert!(config.enable_deep_verify_on_repeated_fail);
    }

    #[test]
    fn test_rule_condition_serialization() {
        let condition = RuleCondition::And(vec![
            RuleCondition::Equals("status".to_string(), serde_json::json!("active")),
            RuleCondition::GreaterThan("level".to_string(), 10.0),
        ]);

        // 验证可以序列化和反序列化
        let serialized = serde_json::to_string(&condition).unwrap();
        let deserialized: RuleCondition = serde_json::from_str(&serialized).unwrap();

        // 验证反序列化后的值与原始值匹配
        match deserialized {
            RuleCondition::And(conditions) => {
                assert_eq!(conditions.len(), 2);
            }
            _ => panic!("Expected And condition"),
        }
    }

    #[test]
    fn test_rule_validation_context_action_type() {
        use crate::models::Intent;
        use uuid::Uuid;

        let agent_id = Uuid::new_v4();
        let intent = Intent::new(
            agent_id,
            1,
            "move",
            Some(serde_json::json!({"target_location": "location_1"})),
        );

        let context = RuleValidationContext {
            intent,
            persona_info: PersonaInfo::default(),
            world_context: String::new(),
            tick_id: 1,
            history_intents: vec![],
            attributes: HashMap::new(),
        };

        assert_eq!(context.action_type().as_str(), "move");
    }

    #[test]
    fn test_rule_validation_context_get_attribute() {
        use crate::models::Intent;
        use uuid::Uuid;

        let agent_id = Uuid::new_v4();
        let intent = Intent::new(agent_id, 1, "idle", None);

        let mut attributes = HashMap::new();
        attributes.insert("health".to_string(), serde_json::json!(100));
        attributes.insert("level".to_string(), serde_json::json!(5));

        let context = RuleValidationContext {
            intent,
            persona_info: PersonaInfo::default(),
            world_context: String::new(),
            tick_id: 1,
            history_intents: vec![],
            attributes,
        };

        assert_eq!(
            context.get_attribute("health"),
            Some(&serde_json::json!(100))
        );
        assert_eq!(context.get_attribute("level"), Some(&serde_json::json!(5)));
        assert_eq!(context.get_attribute("nonexistent"), None);
    }

    #[test]
    fn test_rule_validation_context_from_request() {
        use crate::models::Intent;
        use uuid::Uuid;

        let agent_id = Uuid::new_v4();
        let intent = Intent::new(agent_id, 1, "idle", None);

        let request = ValidationRequest {
            intent,
            persona: PersonaInfo::default(),
            world_context: "test world".to_string(),
        };

        let history_intents = vec![];
        let attributes = HashMap::new();

        let context = RuleValidationContext::from_request(request, history_intents, attributes);

        assert_eq!(context.tick_id, 1);
        assert_eq!(context.world_context, "test world");
        assert!(context.history_intents.is_empty());
        assert!(context.attributes.is_empty());
    }
}
