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

/// 移动目标规范化：LLM 常用叙事短名（如"厨房"）、 AdjacentNode 名称、或复合写法
/// （如"龙门客栈-厨房"）填 target_location，而规则校验与 server 均要求精确 node_id。
///
/// 匹配顺序：精确 node_id → 相邻节点 name → node_id 后缀/前缀/包含。唯一候选命中时
/// 原位改写为精确 node_id（提交给 server 的意图即携带精确 ID，端到端打通）；
/// 零候选或多候选保持原值（照常驳回，驳回反馈中的可达 ID 列表供 self-correction 使用）。
///
/// 返回 Some((旧值, 新值)) 表示发生了改写。
pub fn canonicalize_move_target(intent: &mut Intent, ws: &WorldState) -> Option<(String, String)> {
    if intent.action_type.as_str() != "移动" {
        return None;
    }
    let value = intent
        .action_data
        .as_ref()?
        .get("target_location")?
        .as_str()?
        .trim()
        .to_string();
    if value.is_empty() {
        return None;
    }
    let reachable: Vec<(String, String)> = ws
        .location
        .adjacent_nodes
        .iter()
        .map(|n| (n.node_id.clone(), n.name.clone()))
        .collect();

    // 精确命中：无需改写
    if reachable.iter().any(|(id, _)| *id == value) {
        return None;
    }

    // 模糊候选：名称相等 / node_id 后缀 / 前缀 / 包含
    let mut candidates: Vec<String> = Vec::new();
    for (id, name) in &reachable {
        let name_hit = !name.is_empty() && name == &value;
        if name_hit || id.ends_with(&value) || value.ends_with(id) || value.contains(id) {
            candidates.push(id.clone());
        }
    }
    candidates.dedup();

    match candidates.len() {
        1 => {
            let new_id = candidates.remove(0);
            let old = value;
            if let Some(obj) = intent.action_data.as_mut().and_then(|d| d.as_object_mut()) {
                obj.insert(
                    "target_location".to_string(),
                    serde_json::Value::String(new_id.clone()),
                );
            }
            tracing::info!("移动目标规范化: {} → {}", old, new_id);
            Some((old, new_id))
        }
        0 => None,
        _ => {
            tracing::warn!(
                "移动目标 {:?} 匹配到多个可达地点 {:?}，保持原值驳回",
                value,
                candidates
            );
            None
        }
    }
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
        let intent = Intent::new(agent_id, 1, "休整", None);

        let mut attributes = HashMap::new();
        attributes.insert("health".to_string(), serde_json::json!(100));
        attributes.insert("level".to_string(), serde_json::json!(5));

        let context = RuleValidationContext {
            intent,
            persona_info: PersonaInfo::default(),
            world_context: String::new(),
            tick_id: 1,
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
        let intent = Intent::new(agent_id, 1, "休整", None);

        let request = ValidationRequest {
            intent,
            persona: PersonaInfo::default(),
            world_context: "test world".to_string(),
            world_state: None,
            runtime: crate::soul::reflector::ValidationRuntimeConfig::default(),
        };

        let attributes = HashMap::new();

        let context = RuleValidationContext::from_request(request, attributes);

        assert_eq!(context.tick_id, 1);
        assert_eq!(context.world_context, "test world");
        assert!(context.attributes.is_empty());
    }

    #[test]
    fn test_rule_json_roundtrip() {
        let rules = vec![
            Rule::new(
                "valid_item_id_use".to_string(),
                "用 的 item_id 必须在背包中".to_string(),
                RuleType::ResourceConstraint,
                RuleCondition::Or(vec![
                    RuleCondition::NotEquals(
                        "intent.action_type".to_string(),
                        serde_json::json!("用"),
                    ),
                    RuleCondition::In(
                        "intent.action_data.item_id".to_string(),
                        "available_item_ids".to_string(),
                    ),
                ]),
                "使用物品失败：物品ID无效".to_string(),
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

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "valid_item_id_use");
        assert_eq!(parsed[0].rule_type, RuleType::ResourceConstraint);
        assert!(parsed[0].enabled);
        match &parsed[0].condition {
            RuleCondition::Or(conds) => {
                assert_eq!(conds.len(), 2);
                assert!(
                    matches!(&conds[0], RuleCondition::NotEquals(f, _) if f == "intent.action_type")
                );
                assert!(
                    matches!(&conds[1], RuleCondition::In(f, c) if f == "intent.action_data.item_id" && c == "available_item_ids")
                );
            }
            _ => panic!("Expected Or condition for use rule"),
        }

        assert_eq!(parsed[1].id, "valid_target_node_move");
        assert_eq!(parsed[1].rule_type, RuleType::StateRestriction);

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
                        {"NotEquals": ["intent.action_type", "用"]},
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

    // ===== canonicalize_move_target 单测 =====

    /// 构造带相邻节点的最小 WorldState（仿 state_store 测试模板）
    fn make_ws(adjacent: Vec<(&str, &str)>) -> cyber_jianghu_protocol::WorldState {
        use cyber_jianghu_protocol::{AdjacentNode, Location, WorldTime};
        use std::collections::HashMap;
        cyber_jianghu_protocol::WorldState {
            event_type: "world_state".to_string(),
            tick_id: 1,
            agent_id: Some(uuid::Uuid::new_v4()),
            world_time: WorldTime {
                year: 1,
                month: 1,
                day: 1,
                hour: 3,
                minute: 0,
                second: 0,
                weather: String::new(),
            },
            location: Location {
                node_id: "龙门大堂".to_string(),
                name: "大堂".to_string(),
                node_type: "inn".to_string(),
                adjacent_nodes: adjacent
                    .into_iter()
                    .map(|(id, name)| AdjacentNode {
                        node_id: id.to_string(),
                        name: name.to_string(),
                        travel_cost: 1,
                    })
                    .collect(),
                gatherable_items: vec![],
            },
            self_state: cyber_jianghu_protocol::AgentSelfState {
                attributes: HashMap::new(),
                derived_attributes: HashMap::new(),
                attribute_descriptions: HashMap::new(),
                survival_drives: vec![],
                status_effects: vec![],
                inventory: vec![],
                skills: vec![],
                recipe_details: vec![],
                age_years: None,
                max_age: None,
            },
            entities: vec![],
            nearby_items: vec![],
            events_log: vec![],
            private_dialogue_log: vec![],
            last_execution_summary: None,
        }
    }

    fn make_move_intent(target: &str) -> crate::models::Intent {
        use uuid::Uuid;
        Intent::new(
            Uuid::new_v4(),
            1,
            "移动",
            Some(serde_json::json!({"target_location": target})),
        )
    }

    #[test]
    fn test_canonicalize_short_name_suffix() {
        // 仿龙门客栈地图：node_id 带前缀，LLM 写短名
        let ws = make_ws(vec![
            ("龙门客栈", "客栈"),
            ("龙门后院", "后院"),
            ("龙门厨房", "厨房"),
        ]);
        let mut intent = make_move_intent("厨房");
        let r = canonicalize_move_target(&mut intent, &ws);
        assert_eq!(r, Some(("厨房".to_string(), "龙门厨房".to_string())));
        assert_eq!(
            intent.action_data.unwrap()["target_location"],
            serde_json::json!("龙门厨房")
        );
    }

    #[test]
    fn test_canonicalize_exact_id_no_rewrite() {
        let ws = make_ws(vec![("龙门客栈", "客栈")]);
        let mut intent = make_move_intent("龙门客栈");
        assert_eq!(canonicalize_move_target(&mut intent, &ws), None);
    }

    #[test]
    fn test_canonicalize_adjacent_name_match() {
        let ws = make_ws(vec![("龙门客栈", "客栈")]);
        let mut intent = make_move_intent("客栈");
        assert_eq!(
            canonicalize_move_target(&mut intent, &ws),
            Some(("客栈".to_string(), "龙门客栈".to_string()))
        );
    }

    #[test]
    fn test_canonicalize_compound_write() {
        // 实测案例：LLM 输出"龙门客栈-厨房"复合写法（从厨房回客栈）
        let ws = make_ws(vec![("龙门客栈", "客栈")]);
        let mut intent = make_move_intent("龙门客栈-厨房");
        assert_eq!(
            canonicalize_move_target(&mut intent, &ws),
            Some(("龙门客栈-厨房".to_string(), "龙门客栈".to_string()))
        );
    }

    #[test]
    fn test_canonicalize_no_match_keeps_original() {
        // 实测案例："内院"不是任何可达节点 → 不改写，照常驳回
        let ws = make_ws(vec![
            ("龙门客栈", "客栈"),
            ("龙门后院", "后院"),
            ("龙门厨房", "厨房"),
        ]);
        let mut intent = make_move_intent("内院");
        assert_eq!(canonicalize_move_target(&mut intent, &ws), None);
        assert_eq!(
            intent.action_data.unwrap()["target_location"],
            serde_json::json!("内院")
        );
    }

    #[test]
    fn test_canonicalize_ambiguous_keeps_original() {
        // 名称重名 → 多候选 → 不改写
        let ws = make_ws(vec![("甲院", "院子"), ("乙院", "院子")]);
        let mut intent = make_move_intent("院子");
        assert_eq!(canonicalize_move_target(&mut intent, &ws), None);
    }

    #[test]
    fn test_canonicalize_non_move_ignored() {
        let ws = make_ws(vec![("龙门厨房", "厨房")]);
        use uuid::Uuid;
        let mut intent = Intent::new(
            Uuid::new_v4(),
            1,
            "观察",
            Some(serde_json::json!({"target_location": "厨房"})),
        );
        assert_eq!(canonicalize_move_target(&mut intent, &ws), None);
    }
}
