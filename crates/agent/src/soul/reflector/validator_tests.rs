use super::*;
use crate::component::llm::MockLlmClient;
use crate::soul::reflector::types::ValidationRuntimeConfig;
use cyber_jianghu_protocol::{
    AdjacentNode, AgentSelfState, Entity, GradedValidationConfig, InventoryItem, Location,
    SceneItem, WorldState, WorldTime,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

pub(super) fn mock_container(client: MockLlmClient) -> LlmClientContainer {
    Arc::new(RwLock::new(Arc::new(client)))
}

pub(super) fn test_world_building_rules() -> WorldBuildingRules {
    use cyber_jianghu_protocol::EraSettings;
    WorldBuildingRules {
        version: "0.0.1-test".to_string(),
        era: EraSettings {
            name: "武侠架空世界".to_string(),
            tech_level: "冷兵器时代".to_string(),
            social_structure: "封建帝制".to_string(),
        },
        allowed_concepts: vec!["内力".to_string(), "轻功".to_string()],
        forbidden_concepts: vec!["魔法".to_string()],
        narrative_rules: "测试叙事规则".to_string(),
        last_updated: "2026-01-01T00:00:00Z".to_string(),
        rules_json: None,
        known_item_ids: Vec::new(),
    }
}

pub(super) fn test_world_state() -> WorldState {
    let mut attributes = HashMap::new();
    attributes.insert("satiation".to_string(), 80);
    attributes.insert("hydration".to_string(), 80);

    // 协议层物品标识为完整 uuid（v5 派生），与 broadcaster 下发形态一致
    let mantou_uuid = cyber_jianghu_protocol::item_uuid("馒头").to_string();
    let stick_uuid = cyber_jianghu_protocol::item_uuid("木棍").to_string();

    WorldState {
        event_type: "world_state".to_string(),
        tick_id: 1,
        agent_id: Some(Uuid::new_v4()),
        world_time: WorldTime {
            year: 1,
            month: 1,
            day: 1,
            hour: 8,
            minute: 0,
            second: 0,
            weather: "晴".to_string(),
        },
        location: Location {
            node_id: "loc_a".to_string(),
            name: "地点A".to_string(),
            node_type: "inn".to_string(),
            adjacent_nodes: vec![AdjacentNode {
                node_id: "loc_b".to_string(),
                name: "地点B".to_string(),
                travel_cost: 1,
            }],
            gatherable_items: vec![],
            parent_chain: Vec::new(),
        },
        self_state: AgentSelfState {
            attributes,
            derived_attributes: HashMap::new(),
            attribute_descriptions: HashMap::new(),
            survival_drives: vec![],
            status_effects: vec![],
            // 协议层携带物品 uuid（v5 派生），与新 Server 行为一致
            inventory: vec![InventoryItem {
                item_id: mantou_uuid.clone(),
                name: "馒头".to_string(),
                item_type: "food".to_string(),
                quantity: 1,
                is_equipped: false,
            }],
            skills: vec![],
            age_years: None,
            max_age: None,
            recipe_details: vec![],
        },
        entities: vec![Entity {
            id: Uuid::new_v4(),
            name: "路人甲".to_string(),
            distance: 0,
            state: "alive".to_string(),
            hostile: false,
            recent_actions: vec![],
        }],
        nearby_items: vec![SceneItem {
            item_id: stick_uuid.clone(),
            name: "木棍".to_string(),
            item_type: "weapon".to_string(),
            quantity: 1,
        }],
        events_log: vec![],
        private_dialogue_log: vec![],
        last_execution_summary: None,
    }
}

#[tokio::test]
async fn test_validate_approved() {
    let mock_client = MockLlmClient::with_response(
        r#"{
        "result": "approved",
        "reason": "行为符合武侠世界观",
        "narrative": "李四决定在客栈休息"
    }"#,
    );

    let validator = ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client));

    let request = ValidationRequest {
        intent: crate::models::Intent::new(uuid::Uuid::new_v4(), 1, "休整", None),
        persona: PersonaInfo::default(),
        world_context: "龙门客栈".to_string(),
        world_state: None,
        runtime: ValidationRuntimeConfig::default(),
    };

    let result = validator.validate(request).await.unwrap();

    match result {
        PipelineValidationResult::Approved { narrative, .. } => {
            assert_eq!(narrative, Some("李四决定在客栈休息".to_string()));
        }
        _ => panic!("Expected Approved"),
    }
}

#[tokio::test]
async fn test_validate_rejected() {
    let mock_client = MockLlmClient::with_response(
        r#"{
        "result": "rejected",
        "reason": "使用了魔法，违反力量体系",
        "rejection_type": "power_system_violation"
    }"#,
    );

    let validator = ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client));

    let request = ValidationRequest {
        intent: crate::models::Intent::new(uuid::Uuid::new_v4(), 1, "休整", None),
        persona: PersonaInfo::default(),
        world_context: "龙门客栈".to_string(),
        world_state: None,
        runtime: ValidationRuntimeConfig::default(),
    };

    let result = validator.validate(request).await.unwrap();

    match result {
        PipelineValidationResult::Rejected { reason, .. } => {
            assert_eq!(reason, "使用了魔法，违反力量体系");
        }
        _ => panic!("Expected Rejected"),
    }
}

#[tokio::test]
async fn test_update_rules() {
    let mock_client =
        MockLlmClient::with_response(r#"{"result": "approved", "reason": "", "narrative": ""}"#);

    let validator = ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client));

    // Test that update_rules doesn't panic
    let new_rules = test_world_building_rules();
    validator.update_rules(new_rules).await;
}

#[tokio::test]
async fn test_validator_trait_runs_full_pipeline() {
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    let validator: Arc<dyn Validator> = Arc::new(ReflectorSoul::new(
        test_world_building_rules(),
        mock_container(mock_client),
    ));
    let world_state = test_world_state();
    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "说话",
            Some(serde_json::json!({"content": "你好"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig {
            graded_config: Some(GradedValidationConfig::default()),
            recent_same_type_decisions: vec![],
            acquired_item_ids: vec![],
        },
    };

    match validator.validate(request).await.unwrap() {
        PipelineValidationResult::Approved { layers, .. } => {
            assert!(layers.iter().all(|l| l.passed), "all layers should pass");
        }
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("valid intent should be approved, got: {}", reason);
        }
    }
}

#[tokio::test]
async fn test_layer3_rejects_semantic_repeat() {
    let mock_client = MockLlmClient::with_response(
        r#"{"result":"rejected","reason":"重复自我介绍","rejection_type":"semantic_repeat"}"#,
    );
    let validator = ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client));
    let world_state = test_world_state();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "说话",
            Some(serde_json::json!({"content": "在下张三，行走江湖"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig {
            graded_config: None,
            recent_same_type_decisions: vec![
                "说话：你好，我叫张三".to_string(),
                "说话：在下张三".to_string(),
            ],
            acquired_item_ids: vec![],
        },
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, layers } => {
            assert_eq!(reason, "重复自我介绍");
            let layer3 = layers.last().expect("should have layer3");
            assert_eq!(layer3.layer, "layer3");
            assert!(!layer3.passed);
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("semantic repeat should be rejected");
        }
    }
}

#[tokio::test]
async fn test_no_dedup_section_when_empty_history() {
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    let validator = ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client));
    let world_state = test_world_state();

    // 无历史数据时，prompt 不含去重指令，正常通过
    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "说话",
            Some(serde_json::json!({"content": "初次见面"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig {
            graded_config: None,
            recent_same_type_decisions: vec![],
            acquired_item_ids: vec![],
        },
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Approved { layers, .. } => {
            assert!(layers.iter().all(|l| l.passed));
        }
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("no history should not trigger dedup rejection: {}", reason);
        }
    }
}
