//! Cognitive Context 集成测试
//!
//! 测试 CognitiveContextBuilder 从 WorldState 生成结构化认知上下文

use cyber_jianghu_agent::infra::api::cognitive_context::CognitiveContextBuilder;
use cyber_jianghu_protocol::{
    AdjacentNode, AgentSelfState, Entity, Location, SceneItem, WorldEvent, WorldEventType,
    WorldState, WorldTime,
};
use std::collections::HashMap;
use uuid::Uuid;

fn create_test_world_state() -> WorldState {
    let mut attributes = HashMap::new();
    attributes.insert("hp".to_string(), 80);
    attributes.insert("stamina".to_string(), 60);
    attributes.insert("hunger".to_string(), 70);
    attributes.insert("thirst".to_string(), 50);

    WorldState {
        event_type: "world_state".to_string(),
        tick_id: 100,
        agent_id: Some(Uuid::new_v4()),
        deadline_ms: 50000,
        world_time: WorldTime {
            year: 2024,
            month: 3,
            day: 22,
            hour: 12,
            minute: 30,
            second: 0,
            weather: "晴".to_string(),
        },
        location: Location {
            node_id: "jianghu_tavern".to_string(),
            name: "江湖客栈".to_string(),
            node_type: "indoor".to_string(),
            adjacent_nodes: vec![
                AdjacentNode {
                    node_id: "market".to_string(),
                    name: "集市场".to_string(),
                    travel_cost: 1,
                },
                AdjacentNode {
                    node_id: "dojo".to_string(),
                    name: "武馆".to_string(),
                    travel_cost: 1,
                },
            ],
        },
        self_state: AgentSelfState {
            attributes,
            derived_attributes: HashMap::new(),
            attribute_descriptions: HashMap::from([
                ("hp".to_string(), "生命值 80/100，状态良好".to_string()),
                ("stamina".to_string(), "体力有些消耗".to_string()),
                ("hunger".to_string(), "饥饿感明显".to_string()),
                ("thirst".to_string(), "有些口渴".to_string()),
            ]),
            status_effects: vec![],
            inventory: vec![],
        },
        entities: vec![Entity {
            id: Uuid::new_v4(),
            name: "店小二".to_string(),
            distance: 2,
            state: "idle".to_string(),
            hostile: false,
            recent_actions: vec![],
        }],
        nearby_items: vec![SceneItem {
            item_id: "wine_bottle".to_string(),
            name: "女儿红".to_string(),
            quantity: 1,
            item_type: "drink".to_string(),
        }],
        events_log: vec![WorldEvent {
            event_type: WorldEventType::EnvironmentalChange,
            tick_id: 99,
            description: "天色渐晚".to_string(),
            metadata: serde_json::Value::Null,
        }],
        private_dialogue_log: vec![],
        last_execution_summary: None,
    }
}

#[test]
fn test_build_with_world_state() {
    let world_state = create_test_world_state();
    let builder = CognitiveContextBuilder::default();

    // Inject available_actions directly since WorldState no longer has this field
    let injected_actions = vec![
        cyber_jianghu_protocol::AvailableAction {
            action: "speak".to_string(),
            name: "交谈".to_string(),
            description: "与周围的人交谈".to_string(),
            category: "social".to_string(),
            valid_targets: None,
            required_fields: vec![],
            ooc_risk: "high".to_string(),
        },
        cyber_jianghu_protocol::AvailableAction {
            action: "move".to_string(),
            name: "移动".to_string(),
            description: "移动到其他地点".to_string(),
            category: "movement".to_string(),
            valid_targets: Some(vec!["market".to_string(), "dojo".to_string()]),
            required_fields: vec!["target_location".to_string()],
            ooc_risk: "low".to_string(),
        },
        cyber_jianghu_protocol::AvailableAction {
            action: "use".to_string(),
            name: "使用".to_string(),
            description: "使用物品".to_string(),
            category: "interaction".to_string(),
            valid_targets: Some(vec!["wine_bottle".to_string()]),
            required_fields: vec!["item_id".to_string()],
            ooc_risk: "low".to_string(),
        },
    ];

    let ctx = builder.build_with_actions(&world_state, Some(injected_actions), None, None);

    assert!(!ctx.perception.self_status.is_empty());
    assert!(ctx.perception.environment.contains("江湖客栈"));
}

#[test]
fn test_drive_generation_from_attributes() {
    let world_state = create_test_world_state();
    let builder = CognitiveContextBuilder::default();
    let ctx = builder.build(&world_state);

    assert!(!ctx.motivation.active_drives.is_empty());
    let dominant = &ctx.motivation.dominant_drive;
    assert!(!dominant.is_empty());
}

#[test]
fn test_narrative_engine_integrated() {
    let world_state = create_test_world_state();
    let builder = CognitiveContextBuilder::default();
    let ctx = builder.build(&world_state);

    assert!(
        ctx.perception.self_status.contains("饥饿") || ctx.perception.self_status.contains("口渴")
    );
}
