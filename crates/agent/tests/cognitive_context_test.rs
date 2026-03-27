//! Cognitive Context 集成测试
//!
//! 测试 CognitiveContextBuilder 从 WorldState 生成结构化认知上下文

use cyber_jianghu_agent::runtime::decision::http::cognitive_context::{
    CognitiveContext, CognitiveContextBuilder, Drive,
};
use cyber_jianghu_protocol::{
    AdjacentNode, AgentSelfState, AvailableAction, Entity, Location, SceneItem, WorldEvent,
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
        }],
        nearby_items: vec![SceneItem {
            item_id: "wine_bottle".to_string(),
            name: "女儿红".to_string(),
            quantity: 1,
            item_type: "drink".to_string(),
        }],
        events_log: vec![WorldEvent {
            event_type: "environmental_change".to_string(),
            tick_id: 99,
            description: "天色渐晚".to_string(),
            metadata: serde_json::Value::Null,
        }],
        available_actions: vec![
            AvailableAction {
                action: "speak".to_string(),
                description: "与周围的人交谈".to_string(),
                valid_targets: None,
            },
            AvailableAction {
                action: "move".to_string(),
                description: "移动到其他地点".to_string(),
                valid_targets: Some(vec!["market".to_string(), "dojo".to_string()]),
            },
            AvailableAction {
                action: "use".to_string(),
                description: "使用物品".to_string(),
                valid_targets: Some(vec!["wine_bottle".to_string()]),
            },
        ],
    }
}

#[test]
fn test_cognitive_context_builder_default() {
    let _builder = CognitiveContextBuilder::default();
}

#[test]
fn test_cognitive_context_has_required_fields() {
    let ctx = CognitiveContext::default();
    assert!(!ctx.perception.self_status.is_empty());
    assert!(!ctx.perception.environment.is_empty());
    assert!(!ctx.motivation.dominant_drive.is_empty());
    assert!(!ctx.decision.thinking_prompt.is_empty());
}

#[test]
fn test_drive_serialization_format() {
    let drive = Drive {
        drive: "寻找食物".to_string(),
        intensity: 8,
        reason: "肚子饿了".to_string(),
    };

    let json = serde_json::to_string(&drive).unwrap();
    assert!(json.contains("drive"));
    assert!(json.contains("intensity"));
    assert!(json.contains("reason"));
}

#[test]
fn test_cognitive_context_json_output() {
    let ctx = CognitiveContext::default();
    let json = serde_json::to_string_pretty(&ctx).unwrap();

    assert!(json.contains("perception"));
    assert!(json.contains("motivation"));
    assert!(json.contains("planning"));
    assert!(json.contains("decision"));
}

#[test]
fn test_build_with_world_state() {
    let world_state = create_test_world_state();
    let builder = CognitiveContextBuilder::default();
    let ctx = builder.build(&world_state);

    assert!(!ctx.perception.self_status.is_empty());
    assert!(ctx.perception.environment.contains("江湖客栈"));
    assert!(!ctx.planning.available_actions.is_empty());
    assert_eq!(ctx.planning.available_actions.len(), 3);
    assert_eq!(ctx.planning.available_actions[0].action, "speak");
    assert_eq!(ctx.planning.available_actions[1].action, "move");
    assert_eq!(ctx.planning.available_actions[2].action, "use");
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
