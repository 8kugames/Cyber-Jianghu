//! messages 模块单测（自 messages.rs 外移，内容未改）

use super::*;
use crate::types::Intent;

#[test]
fn test_client_message_serialization() {
    let agent_id = Uuid::nil();
    let intent = Intent::new(agent_id, 1, "休整", None);
    let msg = ClientMessage::from_intent(intent);

    let json = msg.to_json().unwrap();
    println!("Serialized ClientMessage: {}", json);

    // 验证格式 - 应该是扁平化的
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "intent");
    assert_eq!(parsed["tick_id"], 1);
    assert_eq!(parsed["action_type"], "休整");
}

#[test]
fn test_client_message_deserialization() {
    // 使用扁平化格式
    let json = r#"{"type":"intent","tick_id":1,"action_type":"说话","action_data":{"content":"hello"},"priority":5}"#;

    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    match msg {
        ClientMessage::Intent {
            tick_id,
            action_type,
            action_data: _,
            priority,
            ..
        } => {
            assert_eq!(tick_id, 1);
            assert_eq!(action_type, "说话");
            assert_eq!(priority, 5);
        }
        _ => panic!("Unexpected message type"),
    }
}

#[test]
fn test_server_message_registered() {
    let agent_id = Uuid::nil();
    let game_rules = GameRules {
        tick_duration_secs: 60,
        initial_items: vec![],
        survival_actions: vec![],
        available_actions: vec![],
        rebirth_delay_ticks: 0,
        version: "0.0.1".to_string(),
        last_updated: "2024-01-01T00:00:00Z".to_string(),
        intent_batch: None,
        immediate_events: None,
        rebirth_retry_max_attempts: 3,
        rebirth_retry_interval_secs: 30,
        lifespan: None,
        calendar: None,
        daily_summary: None,
        dialogue_context: None,
    };
    let msg = ServerMessage::Registered {
        agent_id,
        game_rules,
        world_building_rules: None,
        is_alive: true,
        agent_name: None,
        narrative_config: None,
        narrative_config_hash: None,
    };

    let json = msg.to_json().unwrap();
    println!("Serialized Registered: {}", json);

    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "registered");
    assert_eq!(parsed["agent_id"], "00000000-0000-0000-0000-000000000000");
    assert_eq!(parsed["tick_duration_secs"], 60);
}

#[test]
fn test_server_message_world_state() {
    let world_state = WorldState {
        event_type: "world_state".to_string(),
        tick_id: 1,
        agent_id: None,
        world_time: crate::types::WorldTime {
            year: 2024,
            month: 3,
            day: 15,
            hour: 12,
            minute: 0,
            second: 0,
            weather: "晴".to_string(),
        },
        location: crate::types::Location {
            node_id: "test".to_string(),
            name: "Test".to_string(),
            node_type: "客栈".to_string(),
            adjacent_nodes: vec![],
            gatherable_items: vec![],
            parent_chain: Vec::new(),
        },
        self_state: crate::types::AgentSelfState {
            attributes: {
                let mut attrs = std::collections::HashMap::new();
                attrs.insert("hp".to_string(), 100);
                attrs.insert("stamina".to_string(), 100);
                attrs.insert("satiation".to_string(), 50);
                attrs.insert("hydration".to_string(), 50);
                attrs
            },
            derived_attributes: std::collections::HashMap::new(),
            attribute_descriptions: std::collections::HashMap::new(),
            survival_drives: vec![],
            status_effects: vec![],
            skills: vec![],
            inventory: vec![],
            age_years: None,
            max_age: None,
            recipe_details: vec![],
        },
        entities: vec![],
        nearby_items: vec![],
        events_log: vec![],
        private_dialogue_log: vec![],
        last_execution_summary: None,
    };

    let msg = ServerMessage::WorldState { data: world_state };
    let json = msg.to_json().unwrap();
    println!("ServerMessage WorldState: {}", json);

    // 验证 flatten 效果
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "world_state");
    assert_eq!(parsed["tick_id"], 1);
}

#[test]
fn test_server_message_pong() {
    let msg = ServerMessage::Pong {
        timestamp: 1234567890,
    };
    let json = msg.to_json().unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "pong");
    assert_eq!(parsed["timestamp"], 1234567890);
}

#[test]
fn test_server_message_error() {
    let msg = ServerMessage::Error {
        code: "unknown".to_string(),
        message: "Something went wrong".to_string(),
        current_tick_id: None,
    };
    let json = msg.to_json().unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "error");
    assert_eq!(parsed["message"], "Something went wrong");
    assert_eq!(parsed["code"], "unknown");
}

#[test]
fn test_server_message_error_no_code() {
    // 不带 code 字段时默认为空字符串
    let json = r#"{"type":"error","message":"Something went wrong"}"#;
    let msg: ServerMessage = serde_json::from_str(json).unwrap();
    match msg {
        ServerMessage::Error {
            code,
            message,
            current_tick_id: _,
        } => {
            assert!(code.is_empty());
            assert_eq!(message, "Something went wrong");
        }
        _ => panic!("Expected Error"),
    }
}

#[test]
fn test_dialogue_message_serialization() {
    let msg = DialogueMessage::Request {
        from_agent_id: Uuid::new_v4(),
        to_agent_id: Uuid::new_v4(),
        opening_remark: "你好，能聊聊吗？".to_string(),
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("request"));

    let parsed: DialogueMessage = serde_json::from_str(&json).unwrap();
    match parsed {
        DialogueMessage::Request { opening_remark, .. } => {
            assert_eq!(opening_remark, "你好，能聊聊吗？");
        }
        _ => panic!("Unexpected message type"),
    }
}

#[test]
fn test_server_message_world_building_rules_update() {
    use crate::types::{EraSettings, WorldBuildingRules};

    let rules = WorldBuildingRules {
        version: "0.0.1-test".to_string(),
        era: EraSettings {
            name: "测试世界".to_string(),
            tech_level: "测试".to_string(),
            social_structure: "测试".to_string(),
        },
        allowed_concepts: vec!["内力".to_string()],
        forbidden_concepts: vec!["魔法".to_string()],
        narrative_rules: "测试叙事规则".to_string(),
        last_updated: "2026-01-01T00:00:00Z".to_string(),
        rules_json: None,
        known_item_ids: Vec::new(),
    };
    let msg = ServerMessage::ConfigUpdate {
        config_type: ConfigType::WorldBuildingRules,
        update_type: "full".to_string(),
        version: rules.version.clone(),
        content: serde_json::to_value(&rules).unwrap(),
        content_hash: None,
        updated_items: vec![],
        removed_items: vec![],
    };

    let json = msg.to_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "config_update");
    assert_eq!(parsed["config_type"], "world_building_rules");
    assert_eq!(parsed["version"], "0.0.1-test");
}

#[test]
fn test_server_message_registered_with_world_building_rules() {
    use crate::types::{EraSettings, WorldBuildingRules};

    let agent_id = Uuid::nil();
    let game_rules = GameRules {
        tick_duration_secs: 60,
        initial_items: vec![],
        survival_actions: vec![],
        available_actions: vec![],
        rebirth_delay_ticks: 0,
        version: "0.0.1".to_string(),
        last_updated: "2024-01-01T00:00:00Z".to_string(),
        intent_batch: None,
        immediate_events: None,
        rebirth_retry_max_attempts: 3,
        rebirth_retry_interval_secs: 30,
        lifespan: None,
        calendar: None,
        daily_summary: None,
        dialogue_context: None,
    };
    let world_rules = WorldBuildingRules {
        version: "0.0.1-test".to_string(),
        era: EraSettings {
            name: "测试世界".to_string(),
            tech_level: "测试".to_string(),
            social_structure: "测试".to_string(),
        },
        allowed_concepts: vec!["内力".to_string()],
        forbidden_concepts: vec!["魔法".to_string()],
        narrative_rules: "测试叙事规则".to_string(),
        last_updated: "2026-01-01T00:00:00Z".to_string(),
        rules_json: None,
        known_item_ids: Vec::new(),
    };

    let msg = ServerMessage::Registered {
        agent_id,
        game_rules,
        world_building_rules: Some(world_rules),
        is_alive: true,
        agent_name: None,
        narrative_config: None,
        narrative_config_hash: None,
    };

    let json = msg.to_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "registered");
    assert!(parsed.get("world_building_rules").is_some());
}

#[test]
fn test_server_message_agent_died() {
    let msg = ServerMessage::AgentDied {
        agent_id: Uuid::nil(),
        cause: "satiation".to_string(),
        description: "因饥饿而死".to_string(),
        location: "tavern".to_string(),
        tick_id: 42,
        died_at: 1234567890000,
        rebirth_delay_ticks: 10,
        metadata: None,
    };

    let json = msg.to_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    // Verify type is serialized as "agent_died" (snake_case)
    assert_eq!(parsed["type"], "agent_died");
    assert_eq!(parsed["agent_id"], "00000000-0000-0000-0000-000000000000");
    assert_eq!(parsed["cause"], "satiation");
    assert_eq!(parsed["description"], "因饥饿而死");
    assert_eq!(parsed["location"], "tavern");
    assert_eq!(parsed["tick_id"], 42);
    assert_eq!(parsed["died_at"], 1234567890000_i64);
    assert_eq!(parsed["rebirth_delay_ticks"], 10);

    // Verify round-trip deserialization
    let deserialized: ServerMessage = ServerMessage::from_json(&json).unwrap();
    match deserialized {
        ServerMessage::AgentDied {
            agent_id,
            cause,
            description,
            location,
            tick_id,
            died_at,
            rebirth_delay_ticks,
            metadata: _,
        } => {
            assert_eq!(agent_id, Uuid::nil());
            assert_eq!(cause, "satiation");
            assert_eq!(description, "因饥饿而死");
            assert_eq!(location, "tavern");
            assert_eq!(tick_id, 42);
            assert_eq!(died_at, 1234567890000);
            assert_eq!(rebirth_delay_ticks, 10);
        }
        _ => panic!("Unexpected message type"),
    }
}
