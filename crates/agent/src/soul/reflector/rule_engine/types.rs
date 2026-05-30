//! 规则类型定义
//!
//! 定义规则引擎使用的所有数据类型。

use crate::models::Intent;
use crate::soul::reflector::types::{PersonaInfo, ValidationRequest};
use cyber_jianghu_protocol::{ActionType, WorldState};
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
    /// 字段值必须在指定集合字段中
    /// In("intent.action_data.item_id", "available_item_ids")
    In(String, String),
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
    /// 历史意图（保留字段，未来可用于上下文感知验证）
    pub history_intents: Vec<Intent>,
    /// 额外的属性数据（用于规则检查）
    pub attributes: HashMap<String, serde_json::Value>,
    /// 可用物品 ID 列表（从 WorldState.inventory 提取）
    pub available_item_ids: Vec<String>,
    /// 可达地点 ID 列表（从 WorldState.location.adjacent_nodes 提取）
    pub reachable_node_ids: Vec<String>,
}

impl RuleValidationContext {
    /// 从 ValidationRequest 创建上下文
    pub fn from_request(
        request: ValidationRequest,
        history_intents: Vec<Intent>,
        attributes: HashMap<String, serde_json::Value>,
    ) -> Self {
        let tick_id = request.intent.tick_id;
        let (available_item_ids, reachable_node_ids) = request
            .world_state
            .as_ref()
            .map(extract_ids_from_world_state)
            .unwrap_or_default();
        Self {
            intent: request.intent,
            persona_info: request.persona,
            world_context: request.world_context,
            tick_id,
            history_intents,
            attributes,
            available_item_ids,
            reachable_node_ids,
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
            enable_resource_constraints: true,
            consecutive_failures_for_deep_verify: 3,
            enable_deep_verify_on_repeated_fail: true,
        }
    }
}

/// Extract valid item IDs and reachable node IDs from WorldState
pub fn extract_ids_from_world_state(ws: &WorldState) -> (Vec<String>, Vec<String>) {
    let items: Vec<String> = ws
        .self_state
        .inventory
        .iter()
        .map(|i| i.item_id.clone())
        .collect();
    let nodes: Vec<String> = ws
        .location
        .adjacent_nodes
        .iter()
        .map(|n| n.node_id.clone())
        .collect();
    (items, nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "移动",
            Some(serde_json::json!({"target_location": "location_1"})),
        );

        let context = RuleValidationContext {
            intent,
            persona_info: PersonaInfo::default(),
            world_context: String::new(),
            tick_id: 1,
            history_intents: vec![],
            attributes: HashMap::new(),
            available_item_ids: vec![],
            reachable_node_ids: vec![],
        };

        assert_eq!(context.action_type().as_str(), "移动");
    }

    #[test]
    fn test_rule_validation_context_get_attribute() {
        use crate::models::Intent;
        use uuid::Uuid;

        let agent_id = Uuid::new_v4();
        let intent = Intent::new(agent_id, 1, "休息", None);

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
            available_item_ids: vec![],
            reachable_node_ids: vec![],
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
        let intent = Intent::new(agent_id, 1, "休息", None);

        let request = ValidationRequest {
            intent,
            persona: PersonaInfo::default(),
            world_context: "test world".to_string(),
            world_state: None,
            runtime: crate::soul::reflector::ValidationRuntimeConfig::default(),
        };

        let history_intents = vec![];
        let attributes = HashMap::new();

        let context = RuleValidationContext::from_request(request, history_intents, attributes);

        assert_eq!(context.tick_id, 1);
        assert_eq!(context.world_context, "test world");
        assert!(context.history_intents.is_empty());
        assert!(context.attributes.is_empty());
    }

    #[test]
    fn test_rule_json_roundtrip() {
        let rules = vec![
            Rule::new(
                "valid_item_id_eat".to_string(),
                "eat 的 item_id 必须在背包中".to_string(),
                RuleType::ResourceConstraint,
                RuleCondition::Or(vec![
                    RuleCondition::NotEquals(
                        "intent.action_type".to_string(),
                        serde_json::json!("进食"),
                    ),
                    RuleCondition::In(
                        "intent.action_data.item_id".to_string(),
                        "available_item_ids".to_string(),
                    ),
                ]),
                "吃东西失败：物品ID无效".to_string(),
            ),
            Rule::new(
                "valid_item_id_drink".to_string(),
                "drink 的 item_id 必须在背包中".to_string(),
                RuleType::ResourceConstraint,
                RuleCondition::Or(vec![
                    RuleCondition::NotEquals(
                        "intent.action_type".to_string(),
                        serde_json::json!("饮水"),
                    ),
                    RuleCondition::In(
                        "intent.action_data.item_id".to_string(),
                        "available_item_ids".to_string(),
                    ),
                ]),
                "喝水失败：物品ID无效".to_string(),
            ),
            Rule::new(
                "valid_target_node_move".to_string(),
                "move 的 target_location 必须可达".to_string(),
                RuleType::StateRestriction,
                RuleCondition::Or(vec![
                    RuleCondition::NotEquals(
                        "intent.action_type".to_string(),
                        serde_json::json!("移动"),
                    ),
                    RuleCondition::In(
                        "intent.action_data.target_location".to_string(),
                        "reachable_node_ids".to_string(),
                    ),
                ]),
                "移动失败：目标地点ID无效".to_string(),
            ),
        ];

        let json = serde_json::to_string_pretty(&rules).unwrap();
        let parsed: Vec<Rule> = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].id, "valid_item_id_eat");
        assert_eq!(parsed[0].rule_type, RuleType::ResourceConstraint);
        assert!(parsed[0].enabled);
        match &parsed[0].condition {
            RuleCondition::Or(conds) => {
                assert_eq!(conds.len(), 2);
                assert!(matches!(&conds[0], RuleCondition::NotEquals(f, _) if f == "intent.action_type"));
                assert!(matches!(&conds[1], RuleCondition::In(f, c) if f == "intent.action_data.item_id" && c == "available_item_ids"));
            }
            _ => panic!("Expected Or condition for eat rule"),
        }

        assert_eq!(parsed[1].id, "valid_item_id_drink");
        assert_eq!(parsed[2].id, "valid_target_node_move");
        assert_eq!(parsed[2].rule_type, RuleType::StateRestriction);

        // 二次 round-trip 确保稳定
        let json2 = serde_json::to_string_pretty(&parsed).unwrap();
        let parsed2: Vec<Rule> = serde_json::from_str(&json2).unwrap();
        assert_eq!(parsed2.len(), parsed.len());
    }

    #[test]
    fn test_rule_json_array_roundtrip() {
        let rules_json: serde_json::Value = serde_json::json!([
            {
                "id": "test_rule",
                "name": "测试规则",
                "rule_type": "ResourceConstraint",
                "condition": {
                    "Or": [
                        {"NotEquals": ["intent.action_type", "进食"]},
                        {"In": ["intent.action_data.item_id", "available_item_ids"]}
                    ]
                },
                "error_message": "测试错误",
                "enabled": true
            }
        ]);

        let rules: Vec<Rule> = serde_json::from_value(rules_json).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "test_rule");
        assert!(matches!(rules[0].condition, RuleCondition::Or(_)));
    }
}
