//! 规则条件评估器
//!
//! 提供规则条件的评估逻辑，支持各种条件类型的判断。

use async_trait::async_trait;
use serde_json::Value;

use super::types::{RuleCondition, RuleValidationContext};

/// 规则条件评估器 Trait
#[async_trait]
pub trait ConditionEvaluator: Send + Sync {
    /// 评估规则条件是否满足
    async fn evaluate(&self, condition: &RuleCondition, context: &RuleValidationContext) -> bool;
}

/// 默认规则条件评估器
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultEvaluator;

#[async_trait]
impl ConditionEvaluator for DefaultEvaluator {
    async fn evaluate(&self, condition: &RuleCondition, context: &RuleValidationContext) -> bool {
        match condition {
            RuleCondition::Equals(field, expected) => {
                self.evaluate_equals(field, expected, context).await
            }
            RuleCondition::NotEquals(field, expected) => {
                self.evaluate_not_equals(field, expected, context).await
            }
            RuleCondition::GreaterThan(field, threshold) => {
                self.evaluate_greater_than(field, *threshold, context).await
            }
            RuleCondition::LessThan(field, threshold) => {
                self.evaluate_less_than(field, *threshold, context).await
            }
            RuleCondition::Contains(field, value) => {
                self.evaluate_contains(field, value, context).await
            }
            RuleCondition::NotContains(field, value) => {
                self.evaluate_not_contains(field, value, context).await
            }
            RuleCondition::And(conditions) => self.evaluate_and(conditions, context).await,
            RuleCondition::Or(conditions) => self.evaluate_or(conditions, context).await,
            RuleCondition::Not(condition) => self.evaluate_not(condition, context).await,
            RuleCondition::In(field, collection) => {
                self.evaluate_in(field, collection, context).await
            }
        }
    }
}

impl DefaultEvaluator {
    /// 从上下文中获取字段值
    ///
    /// 支持的点号路径：
    /// - "intent.action_type" -> intent.action_type.as_str()
    /// - "intent.action_data.xxx" -> intent.action_data["xxx"]
    /// - "attributes.xxx" -> context.attributes["xxx"]
    /// - "tick_id" -> context.tick_id
    fn get_field_value(&self, field_path: &str, context: &RuleValidationContext) -> Option<Value> {
        let parts: Vec<&str> = field_path.split('.').collect();

        match parts.first() {
            Some(&"intent") => {
                if parts.len() < 2 {
                    return None;
                }
                match parts.get(1) {
                    Some(&"action_type") => Some(Value::String(
                        context.intent.action_type.as_str().to_string(),
                    )),
                    Some(&"action_data") => {
                        if parts.len() < 3 {
                            context.intent.action_data.clone()
                        } else {
                            context
                                .intent
                                .action_data
                                .as_ref()
                                .and_then(|data| data.get(parts[2]).cloned())
                        }
                    }
                    Some(&"tick_id") => Some(Value::Number(context.intent.tick_id.into())),
                    Some(&"priority") => Some(Value::Number(context.intent.priority.into())),
                    _ => None,
                }
            }
            Some(&"attributes") => {
                if parts.len() < 2 {
                    return None;
                }
                context.attributes.get(parts[1]).cloned()
            }
            Some(&"tick_id") => Some(Value::Number(context.tick_id.into())),
            _ => None,
        }
    }

    /// 评估等于条件
    async fn evaluate_equals(
        &self,
        field: &str,
        expected: &Value,
        context: &RuleValidationContext,
    ) -> bool {
        match self.get_field_value(field, context) {
            Some(actual) => {
                // 尝试直接比较
                if actual == *expected {
                    return true;
                }

                // 尝试数值比较（处理字符串形式的数字）
                match (&actual, expected) {
                    (Value::String(a_str), Value::Number(_)) => {
                        if let Ok(a_num) = a_str.parse::<f64>()
                            && let Some(exp_num) = expected.as_f64()
                        {
                            return (a_num - exp_num).abs() < f64::EPSILON;
                        }
                    }
                    (Value::Number(_), Value::String(e_str)) => {
                        if let Ok(e_num) = e_str.parse::<f64>()
                            && let Some(act_num) = actual.as_f64()
                        {
                            return (act_num - e_num).abs() < f64::EPSILON;
                        }
                    }
                    _ => {}
                }

                false
            }
            None => false,
        }
    }

    /// 评估不等于条件
    async fn evaluate_not_equals(
        &self,
        field: &str,
        expected: &Value,
        context: &RuleValidationContext,
    ) -> bool {
        !self.evaluate_equals(field, expected, context).await
    }

    /// 评估大于条件
    async fn evaluate_greater_than(
        &self,
        field: &str,
        threshold: f64,
        context: &RuleValidationContext,
    ) -> bool {
        match self.get_field_value(field, context) {
            Some(Value::Number(n)) => {
                if let Some(value) = n.as_f64() {
                    value > threshold
                } else {
                    false
                }
            }
            Some(Value::String(s)) => {
                if let Ok(value) = s.parse::<f64>() {
                    value > threshold
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// 评估小于条件
    async fn evaluate_less_than(
        &self,
        field: &str,
        threshold: f64,
        context: &RuleValidationContext,
    ) -> bool {
        match self.get_field_value(field, context) {
            Some(Value::Number(n)) => {
                if let Some(value) = n.as_f64() {
                    value < threshold
                } else {
                    false
                }
            }
            Some(Value::String(s)) => {
                if let Ok(value) = s.parse::<f64>() {
                    value < threshold
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// 评估包含条件
    async fn evaluate_contains(
        &self,
        field: &str,
        value: &str,
        context: &RuleValidationContext,
    ) -> bool {
        match self.get_field_value(field, context) {
            Some(Value::String(s)) => s.contains(value),
            Some(Value::Array(arr)) => {
                // 检查数组是否包含该值（作为字符串或数字）
                arr.iter().any(|item| match item {
                    Value::String(s) => s.contains(value),
                    Value::Number(n) => {
                        if let Some(num) = n.as_f64() {
                            value == num.to_string()
                        } else {
                            false
                        }
                    }
                    _ => false,
                })
            }
            _ => false,
        }
    }

    /// 评估不包含条件
    async fn evaluate_not_contains(
        &self,
        field: &str,
        value: &str,
        context: &RuleValidationContext,
    ) -> bool {
        !self.evaluate_contains(field, value, context).await
    }

    /// 评估且（AND）条件
    async fn evaluate_and(
        &self,
        conditions: &[RuleCondition],
        context: &RuleValidationContext,
    ) -> bool {
        for condition in conditions {
            if !self.evaluate(condition, context).await {
                return false;
            }
        }
        true
    }

    /// 评估或（OR）条件
    async fn evaluate_or(
        &self,
        conditions: &[RuleCondition],
        context: &RuleValidationContext,
    ) -> bool {
        for condition in conditions {
            if self.evaluate(condition, context).await {
                return true;
            }
        }
        false
    }

    /// 评估非（NOT）条件
    async fn evaluate_not(
        &self,
        condition: &RuleCondition,
        context: &RuleValidationContext,
    ) -> bool {
        !self.evaluate(condition, context).await
    }

    /// 评估 In 条件：字段值必须在指定集合中
    async fn evaluate_in(
        &self,
        field_path: &str,
        collection_field: &str,
        context: &RuleValidationContext,
    ) -> bool {
        let field_value = match self.get_field_value(field_path, context) {
            Some(v) => v,
            None => return false,
        };

        let field_str = match field_value {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        };

        let collection: &[String] = match collection_field {
            "available_item_ids" => &context.available_item_ids,
            "reachable_node_ids" => &context.reachable_node_ids,
            _ => return false,
        };

        collection.contains(&field_str)
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
            attributes,
            available_item_ids: vec![],
            reachable_node_ids: vec![],
        }
    }

    #[tokio::test]
    async fn test_evaluate_equals_action_type() {
        let evaluator = DefaultEvaluator;
        let context = create_test_context();

        // 测试匹配的动作类型
        let condition =
            RuleCondition::Equals("intent.action_type".to_string(), serde_json::json!("说话"));
        assert!(evaluator.evaluate(&condition, &context).await);

        // 测试不匹配的动作类型
        let condition =
            RuleCondition::Equals("intent.action_type".to_string(), serde_json::json!("移动"));
        assert!(!evaluator.evaluate(&condition, &context).await);
    }

    #[tokio::test]
    async fn test_evaluate_equals_attributes() {
        let evaluator = DefaultEvaluator;
        let context = create_test_context();

        // 测试匹配的属性值
        let condition =
            RuleCondition::Equals("attributes.health".to_string(), serde_json::json!(100));
        assert!(evaluator.evaluate(&condition, &context).await);

        // 测试不匹配的属性值
        let condition =
            RuleCondition::Equals("attributes.health".to_string(), serde_json::json!(50));
        assert!(!evaluator.evaluate(&condition, &context).await);
    }

    #[tokio::test]
    async fn test_evaluate_greater_than() {
        let evaluator = DefaultEvaluator;
        let context = create_test_context();

        // 测试大于条件（通过）
        let condition = RuleCondition::GreaterThan("attributes.health".to_string(), 50.0);
        assert!(evaluator.evaluate(&condition, &context).await);

        // 测试大于条件（不通过）
        let condition = RuleCondition::GreaterThan("attributes.health".to_string(), 150.0);
        assert!(!evaluator.evaluate(&condition, &context).await);
    }

    #[tokio::test]
    async fn test_evaluate_less_than() {
        let evaluator = DefaultEvaluator;
        let context = create_test_context();

        // 测试小于条件（通过）
        let condition = RuleCondition::LessThan("attributes.health".to_string(), 150.0);
        assert!(evaluator.evaluate(&condition, &context).await);

        // 测试小于条件（不通过）
        let condition = RuleCondition::LessThan("attributes.health".to_string(), 50.0);
        assert!(!evaluator.evaluate(&condition, &context).await);
    }

    #[tokio::test]
    async fn test_evaluate_contains() {
        let evaluator = DefaultEvaluator;
        let context = create_test_context();

        // 测试包含条件（action_data 中的字符串）
        let condition = RuleCondition::Contains(
            "intent.action_data.content".to_string(),
            "hello".to_string(),
        );
        assert!(evaluator.evaluate(&condition, &context).await);

        // 测试包含条件（不通过）
        let condition = RuleCondition::Contains(
            "intent.action_data.content".to_string(),
            "goodbye".to_string(),
        );
        assert!(!evaluator.evaluate(&condition, &context).await);
    }

    #[tokio::test]
    async fn test_evaluate_logical_operators() {
        let evaluator = DefaultEvaluator;
        let context = create_test_context();

        // 测试 AND 条件（通过）
        let condition = RuleCondition::And(vec![
            RuleCondition::Equals("intent.action_type".to_string(), serde_json::json!("说话")),
            RuleCondition::GreaterThan("attributes.health".to_string(), 50.0),
        ]);
        assert!(evaluator.evaluate(&condition, &context).await);

        // 测试 AND 条件（不通过）
        let condition = RuleCondition::And(vec![
            RuleCondition::Equals("intent.action_type".to_string(), serde_json::json!("移动")),
            RuleCondition::GreaterThan("attributes.health".to_string(), 50.0),
        ]);
        assert!(!evaluator.evaluate(&condition, &context).await);

        // 测试 OR 条件（通过）
        let condition = RuleCondition::Or(vec![
            RuleCondition::Equals("intent.action_type".to_string(), serde_json::json!("移动")),
            RuleCondition::Equals("intent.action_type".to_string(), serde_json::json!("说话")),
        ]);
        assert!(evaluator.evaluate(&condition, &context).await);

        // 测试 NOT 条件
        let condition = RuleCondition::Not(Box::new(RuleCondition::Equals(
            "intent.action_type".to_string(),
            serde_json::json!("移动"),
        )));
        assert!(evaluator.evaluate(&condition, &context).await);
    }

    #[tokio::test]
    async fn test_evaluate_in_item_ids() {
        let evaluator = DefaultEvaluator;
        let mut context = create_test_context();
        context.available_item_ids = vec!["馒头".to_string(), "水".to_string()];

        // item_id 在列表中
        let condition = RuleCondition::In(
            "intent.action_data.item_id".to_string(),
            "available_item_ids".to_string(),
        );
        context.intent = Intent::new(
            Uuid::new_v4(),
            10,
            ActionType::SPEAK,
            Some(serde_json::json!({"item_id": "馒头"})),
        );
        assert!(evaluator.evaluate(&condition, &context).await);

        // item_id 不在列表中
        context.intent = Intent::new(
            Uuid::new_v4(),
            10,
            ActionType::SPEAK,
            Some(serde_json::json!({"item_id": "steamed_bun"})),
        );
        assert!(!evaluator.evaluate(&condition, &context).await);
    }

    #[tokio::test]
    async fn test_evaluate_in_node_ids() {
        let evaluator = DefaultEvaluator;
        let mut context = create_test_context();
        context.reachable_node_ids = vec!["龙门厨房".to_string(), "龙门后院".to_string()];

        let condition = RuleCondition::In(
            "intent.action_data.target_location".to_string(),
            "reachable_node_ids".to_string(),
        );

        context.intent = Intent::new(
            Uuid::new_v4(),
            10,
            ActionType::SPEAK,
            Some(serde_json::json!({"target_location": "龙门厨房"})),
        );
        assert!(evaluator.evaluate(&condition, &context).await);

        context.intent = Intent::new(
            Uuid::new_v4(),
            10,
            ActionType::SPEAK,
            Some(serde_json::json!({"target_location": "kitchen"})),
        );
        assert!(!evaluator.evaluate(&condition, &context).await);
    }

    #[tokio::test]
    async fn test_evaluate_in_empty_collection() {
        let evaluator = DefaultEvaluator;
        let context = create_test_context(); // empty available_item_ids

        let condition = RuleCondition::In(
            "intent.action_data.item_id".to_string(),
            "available_item_ids".to_string(),
        );
        assert!(!evaluator.evaluate(&condition, &context).await);
    }

    #[tokio::test]
    async fn test_evaluate_in_unknown_collection() {
        let evaluator = DefaultEvaluator;
        let context = create_test_context();

        let condition = RuleCondition::In(
            "intent.action_data.item_id".to_string(),
            "unknown_collection".to_string(),
        );
        assert!(!evaluator.evaluate(&condition, &context).await);
    }
}
