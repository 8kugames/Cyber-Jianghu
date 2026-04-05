//! 规则引擎核心
//!
//! 提供规则验证的统一入口点，协调注册表和评估器。

use super::evaluator::{ConditionEvaluator, DefaultEvaluator};
use super::registry::{RuleRegistry, RuleSet};
use super::types::{Rule, RuleValidationContext};
use crate::soul::reflector::{
    PersonaInfo, RejectionType, ValidationRequest, ValidationResult, Validator,
};
use async_trait::async_trait;
use cyber_jianghu_protocol::WorldBuildingRules;
use std::collections::HashMap;
use std::sync::Arc;

/// 规则引擎
///
/// 协调规则注册表和条件评估器，提供统一的验证入口
pub struct RuleEngine {
    /// 规则注册表
    registry: Arc<RuleRegistry>,
    /// 条件评估器
    evaluator: Box<dyn ConditionEvaluator>,
}

#[async_trait]
impl Validator for RuleEngine {
    async fn validate(&self, request: ValidationRequest) -> anyhow::Result<ValidationResult> {
        // 构建验证上下文
        let tick_id = request.intent.tick_id;
        let context = RuleValidationContext {
            intent: request.intent,
            persona_info: request.persona,
            world_context: request.world_context,
            tick_id,
            // 注意：RuleValidationContext 的 history_intents 和 attributes 字段暂未填充
            // 当前规则引擎在空数据上下文中进行验证，结果可能不准确
            // 如需启用规则验证，需从 WorldState/AgentState 获取这些数据并传入
            history_intents: vec![],
            attributes: HashMap::new(),
        };

        // 调用内部验证逻辑
        self.validate_context(&context).await
    }

    async fn validate_persona(&self, _persona: &PersonaInfo) -> anyhow::Result<ValidationResult> {
        // 规则引擎暂时不验证人设，直接通过
        Ok(ValidationResult::Approved {
            reason: None,
            narrative: String::new(),
        })
    }

    async fn update_rules(&self, _rules: WorldBuildingRules) {
        // 规则引擎暂时不响应世界观规则更新
        // 未来可以根据世界观规则动态调整验证规则
    }
}

impl RuleEngine {
    /// 创建新的规则引擎（使用默认评估器）
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RuleRegistry::new()),
            evaluator: Box::new(DefaultEvaluator),
        }
    }

    /// 创建带有默认配置的规则引擎（兼容性方法）
    ///
    /// 预加载默认的验证规则（硬编码，未来从 YAML 配置加载）：
    /// - cooldown_speak: speak 冷却检查（需 history_intents 数据）
    /// - cooldown_move: move 冷却检查（需 history_intents 数据）
    ///
    /// 注意：当前 history_intents 为空，冷却规则暂不触发。
    /// 当 decision pipeline 接入历史意图数据后自动生效。
    pub fn with_default_config() -> Self {
        let mut rule_set = RuleSet::new();

        // speak 冷却规则
        rule_set.add_rule(Rule::new(
            "cooldown_speak".to_string(),
            "说话冷却: 连续说话需间隔".to_string(),
            super::types::RuleType::ActionCooldown,
            super::types::RuleCondition::And(vec![
                super::types::RuleCondition::Equals(
                    "intent.action_type".to_string(),
                    serde_json::json!("speak"),
                ),
                super::types::RuleCondition::GreaterThan("cooldown_speak".to_string(), 0.0),
            ]),
            "刚说过话，等一会儿再说".to_string(),
        ));

        // move 冷却规则
        rule_set.add_rule(Rule::new(
            "cooldown_move".to_string(),
            "移动冷却: 连续移动需间隔".to_string(),
            super::types::RuleType::ActionCooldown,
            super::types::RuleCondition::And(vec![
                super::types::RuleCondition::Equals(
                    "intent.action_type".to_string(),
                    serde_json::json!("move"),
                ),
                super::types::RuleCondition::GreaterThan("cooldown_move".to_string(), 0.0),
            ]),
            "刚移动过，休息一下再走".to_string(),
        ));

        Self {
            registry: Arc::new(RuleRegistry::from_rule_set(rule_set)),
            evaluator: Box::new(DefaultEvaluator),
        }
    }

    /// 使用自定义评估器创建规则引擎
    pub fn with_evaluator<E>(mut self, evaluator: E) -> Self
    where
        E: ConditionEvaluator + 'static,
    {
        self.evaluator = Box::new(evaluator);
        self
    }

    /// 获取规则注册表的引用
    pub fn registry(&self) -> Arc<RuleRegistry> {
        Arc::clone(&self.registry)
    }

    /// 验证意图（内部方法）
    ///
    /// 对所有启用的规则进行验证，如果任何规则失败则返回 Rejected
    pub async fn validate_context(
        &self,
        context: &RuleValidationContext,
    ) -> anyhow::Result<ValidationResult> {
        // 获取所有启用的规则
        let rules = self.registry.all_enabled().await;

        tracing::debug!("开始验证，共 {} 条规则", rules.len());

        // 如果没有规则，直接通过
        if rules.is_empty() {
            tracing::debug!("没有启用的规则，直接通过验证");
            return Ok(ValidationResult::Approved {
                reason: None,
                narrative: String::new(),
            });
        }

        // 逐条评估规则
        for rule in &rules {
            let rule_result = self.evaluate_rule(rule, context).await?;

            if !rule_result.passed {
                tracing::warn!(
                    "规则验证失败: {} - {}",
                    rule.id,
                    rule_result
                        .error_message
                        .as_ref()
                        .unwrap_or(&"未知错误".to_string())
                );

                // 规则失败，返回 Rejected
                return Ok(ValidationResult::Rejected {
                    reason: rule_result
                        .error_message
                        .unwrap_or_else(|| format!("规则 {} 验证失败", rule.name)),
                    rejection_type: RejectionType::Other,
                });
            }

            tracing::debug!("规则验证通过: {}", rule.id);
        }

        // 所有规则通过
        Ok(ValidationResult::Approved {
            reason: Some(format!("所有 {} 条规则验证通过", rules.len())),
            narrative: String::new(),
        })
    }

    /// 评估单个规则
    pub async fn evaluate_rule(
        &self,
        rule: &Rule,
        context: &RuleValidationContext,
    ) -> anyhow::Result<super::types::RuleValidationResult> {
        // 防御性检查：跳过未启用的规则
        if !rule.enabled {
            tracing::debug!("规则已禁用，跳过评估: {}", rule.id);
            return Ok(super::types::RuleValidationResult::passed(rule.id.clone()));
        }

        // 使用评估器评估规则条件
        let passed = self.evaluator.evaluate(&rule.condition, context).await;

        if passed {
            Ok(super::types::RuleValidationResult::passed(rule.id.clone()))
        } else {
            Ok(super::types::RuleValidationResult::failed(
                rule.id.clone(),
                rule.error_message.clone(),
            ))
        }
    }
}

impl Default for RuleEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Intent;
    use crate::soul::reflector::types::PersonaInfo;
    use cyber_jianghu_protocol::ActionType;
    use std::collections::HashMap;
    use uuid::Uuid;

    /// 创建测试用的验证上下文
    fn create_test_context() -> RuleValidationContext {
        let agent_id = Uuid::new_v4();
        let intent = Intent::new(
            agent_id,
            10,
            ActionType::SPEAK,
            Some(serde_json::json!({"content": "hello world"})),
        );

        let mut attributes = HashMap::new();
        attributes.insert("health".to_string(), serde_json::json!(100));
        attributes.insert("level".to_string(), serde_json::json!(5));

        RuleValidationContext {
            intent,
            persona_info: PersonaInfo::default(),
            world_context: String::new(),
            tick_id: 10,
            history_intents: vec![],
            attributes,
        }
    }

    #[tokio::test]
    async fn test_validate_no_rules() {
        let engine = RuleEngine::new();
        let context = create_test_context();

        // 没有规则时应该直接通过
        let result = engine.validate_context(&context).await.unwrap();

        match result {
            ValidationResult::Approved { reason, narrative } => {
                assert!(reason.is_none());
                assert!(narrative.is_empty());
            }
            ValidationResult::Rejected { .. } => panic!("应该通过验证，但被拒绝了"),
        }
    }

    #[tokio::test]
    async fn test_validate_failing_rule() {
        let engine = RuleEngine::new();
        let registry = engine.registry();

        // 注册一个会失败的规则（动作类型不是 "move"）
        let rule = Rule::new(
            "test_rule_1".to_string(),
            "动作必须是 move".to_string(),
            super::super::types::RuleType::ActionCooldown,
            super::super::types::RuleCondition::Equals(
                "intent.action_type".to_string(),
                serde_json::json!("move"),
            ),
            "动作类型必须是 move".to_string(),
        );

        registry.register(rule).await;

        let context = create_test_context();
        let result = engine.validate_context(&context).await.unwrap();

        match result {
            ValidationResult::Approved { .. } => {
                panic!("应该被拒绝，但通过了验证");
            }
            ValidationResult::Rejected { reason, .. } => {
                assert!(reason.contains("move") || reason.contains("动作类型"));
            }
        }
    }

    #[tokio::test]
    async fn test_validate_passing_rule() {
        let engine = RuleEngine::new();
        let registry = engine.registry();

        // 注册一个会通过的规则（动作类型是 "speak"）
        let rule = Rule::new(
            "test_rule_2".to_string(),
            "动作必须是 speak".to_string(),
            super::super::types::RuleType::ActionCooldown,
            super::super::types::RuleCondition::Equals(
                "intent.action_type".to_string(),
                serde_json::json!("speak"),
            ),
            "动作类型必须是 speak".to_string(),
        );

        registry.register(rule).await;

        let context = create_test_context();
        let result = engine.validate_context(&context).await.unwrap();

        match result {
            ValidationResult::Approved { reason, .. } => {
                assert!(reason.is_some());
                assert!(reason.as_ref().unwrap().contains("通过"));
            }
            ValidationResult::Rejected { reason, .. } => {
                panic!("应该通过验证，但被拒绝了: {}", reason);
            }
        }
    }
}
