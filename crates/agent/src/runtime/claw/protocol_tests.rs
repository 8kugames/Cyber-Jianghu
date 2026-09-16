//! protocol 模块单测（自 protocol.rs 外移，内容未改）

use super::*;

fn create_test_world_state() -> WorldState {
    // 使用 JSON 构造测试数据，避免直接构造复杂结构
    let json = serde_json::json!({
        "event_type": "world_state",
        "tick_id": 105,
        "agent_id": "00000000-0000-0000-0000-000000000000",
        "world_time": {
            "year": 2024,
            "month": 1,
            "day": 1,
            "hour": 12,
            "minute": 0,
            "second": 0,
            "weather": "晴"
        },
        "location": {
            "node_id": "test",
            "name": "测试地点",
            "type": "indoor",
            "adjacent_nodes": []
        },
        "self_state": {
            "attributes": {},
            "attribute_descriptions": {},
            "status_effects": []
        },
        "entities": [],
        "nearby_items": [],
        "events_log": [],
        "available_actions": []
    });
    serde_json::from_value(json).unwrap()
}

// === 新增消息类型测试 ===

#[test]
fn test_serialize_server_error_agent_dead() {
    let msg = DownstreamMessage::ServerError {
        code: ServerErrorCode::AgentDead,
        message: "Agent 已死亡，无法执行此动作。".to_string(),
        tick_id: Some(105),
        current_tick: Some(110),
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"server_error""#));
    assert!(json.contains(r#""code":"agent_dead""#));
    assert!(json.contains(r#""tick_id":105"#));
    assert!(json.contains(r#""current_tick":110"#));
}

#[test]
fn test_serialize_server_error_rate_limited() {
    let msg = DownstreamMessage::ServerError {
        code: ServerErrorCode::RateLimited,
        message: "Rate limit exceeded.".to_string(),
        tick_id: None,
        current_tick: Some(100),
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"server_error""#));
    assert!(json.contains(r#""code":"rate_limited""#));
    assert!(!json.contains(r#""tick_id""#)); // None 时不序列化
    assert!(json.contains(r#""current_tick":100"#));
}

#[test]
fn test_serialize_server_dialogue_request() {
    let msg = DownstreamMessage::ServerDialogue {
        dialogue_type: "request".to_string(),
        from_agent_id: Uuid::nil(),
        to_agent_id: Some(Uuid::nil()),
        session_id: None,
        opening_remark: Some("少侠，可否借一步说话？".to_string()),
        content: None,
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"server_dialogue""#));
    assert!(json.contains(r#""dialogue_type":"request""#));
    assert!(json.contains(r#""opening_remark""#));
}

#[test]
fn test_serialize_server_game_rules_update() {
    let msg = DownstreamMessage::ServerGameRulesUpdate {
        tick_duration_secs: 60,
        version: "0.0.5".to_string(),
        last_updated: "2024-03-22T10:00:00Z".to_string(),
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"server_game_rules_update""#));
    assert!(json.contains(r#""tick_duration_secs":60"#));
}

#[test]
fn test_serialize_missed_messages() {
    let msg = DownstreamMessage::MissedMessages {
        count: 3,
        suggest_resync: false,
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"missed_messages""#));
    assert!(json.contains(r#""count":3"#));
    assert!(json.contains(r#""suggest_resync":false"#));
}

// === 原有测试 ===

#[test]
fn test_serialize_tick_message() {
    let state = create_test_world_state();

    let msg = DownstreamMessage::Tick {
        tick_id: 105,
        state,
        context: None,
        cognitive_context: None,
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"tick""#));
    assert!(json.contains(r#""tick_id":105"#));
    assert!(!json.contains(r#""context""#)); // None 时不序列化
    assert!(!json.contains(r#""cognitive_context""#)); // None 时不序列化
}

#[test]
fn test_serialize_tick_message_with_context() {
    use crate::infra::api::cognitive_context::{
        CognitiveContext, DecisionContext, MotivationContext, PerceptionContext, PlanningContext,
    };

    let state = create_test_world_state();

    // 创建结构化认知上下文
    let cognitive_context = CognitiveContext {
        perception: PerceptionContext {
            self_status: "身体状态良好".to_string(),
            environment: "长安城东市".to_string(),
            key_observations: vec!["附近有商人".to_string()],
        },
        motivation: MotivationContext {
            active_drives: vec![],
            dominant_drive: "保持现状".to_string(),
        },
        planning: PlanningContext {
            current_goals: vec!["继续当前活动".to_string()],
            available_actions: vec![],
        },
        decision: DecisionContext {
            requires_reasoning: true,
            thinking_prompt: "请决定下一步行动".to_string(),
        },
    };

    let msg = DownstreamMessage::Tick {
        tick_id: 105,
        state,
        context: Some("## 游戏状态上下文\n\n测试上下文".to_string()),
        cognitive_context: Some(cognitive_context),
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"tick""#));
    assert!(json.contains(r#""tick_id":105"#));
    assert!(json.contains(r#""context""#)); // 有 context 字段
    assert!(json.contains(r#""cognitive_context""#)); // 有 cognitive_context 字段
    assert!(json.contains(r#""perception""#));
    assert!(json.contains(r#""motivation""#));
    assert!(json.contains(r#""planning""#));
    assert!(json.contains(r#""decision""#));
}

#[test]
fn test_serialize_tick_closed_message() {
    let msg = DownstreamMessage::TickClosed {
        tick_id: 105,
        reason: "timeout".to_string(),
        next_tick_in_ms: 60000,
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"tick_closed""#));
    assert!(json.contains(r#""reason":"timeout""#));
}

#[test]
fn test_deserialize_intent_message() {
    let json = r#"{"type":"intent","tick_id":105,"action_type":"移动","action_data":{"target":"kitchen"}}"#;
    let msg: UpstreamMessage = serde_json::from_str(json).unwrap();

    match msg {
        UpstreamMessage::Intent {
            tick_id,
            action_type,
            action_data,
            thought_log,
            ..
        } => {
            assert_eq!(tick_id, 105);
            assert_eq!(action_type, "移动");
            assert!(action_data.is_some());
            assert!(thought_log.is_none());
        }
        _ => panic!("Expected Intent message"),
    }
}

#[test]
fn test_deserialize_intent_with_thought() {
    let json = r#"{"type":"intent","tick_id":105,"action_type":"说话","thought_log":"I should greet them"}"#;
    let msg: UpstreamMessage = serde_json::from_str(json).unwrap();

    match msg {
        UpstreamMessage::Intent {
            tick_id,
            action_type,
            action_data,
            thought_log,
            ..
        } => {
            assert_eq!(tick_id, 105);
            assert_eq!(action_type, "说话");
            assert!(action_data.is_none());
            assert_eq!(thought_log, Some("I should greet them".to_string()));
        }
        _ => panic!("Expected Intent message"),
    }
}

#[test]
fn test_deserialize_review_result_approved() {
    let json = r#"{"type":"review_result","tick_id":105,"decision":"approved","reason":"符合角色性格","narrative":"张三热情地向店小二打招呼"}"#;
    let msg: UpstreamMessage = serde_json::from_str(json).unwrap();

    match msg {
        UpstreamMessage::ReviewResult {
            tick_id,
            decision,
            reason,
            narrative,
        } => {
            assert_eq!(tick_id, 105);
            assert!(matches!(decision, ReviewDecision::Approved));
            assert_eq!(reason, Some("符合角色性格".to_string()));
            assert_eq!(narrative, Some("张三热情地向店小二打招呼".to_string()));
        }
        _ => panic!("Expected ReviewResult message"),
    }
}

#[test]
fn test_deserialize_review_result_rejected() {
    let json = r#"{"type":"review_result","tick_id":105,"decision":"rejected"}"#;
    let msg: UpstreamMessage = serde_json::from_str(json).unwrap();

    match msg {
        UpstreamMessage::ReviewResult {
            tick_id,
            decision,
            reason,
            narrative,
        } => {
            assert_eq!(tick_id, 105);
            assert!(matches!(decision, ReviewDecision::Rejected));
            assert!(reason.is_none());
            assert!(narrative.is_none());
        }
        _ => panic!("Expected ReviewResult message"),
    }
}

// === from_server_message 转换测试 ===

#[test]
fn test_from_server_message_error() {
    let server_msg = ServerMessage::Error {
        code: cyber_jianghu_protocol::ERROR_CODE_AGENT_DEAD.to_string(),
        message: "Agent 已死亡，无法执行此动作。".to_string(),
        current_tick_id: None,
    };

    let result = DownstreamMessage::from_server_message(server_msg, 100);
    assert!(result.is_some());

    match result.unwrap() {
        DownstreamMessage::ServerError {
            code,
            message,
            tick_id,
            current_tick,
        } => {
            assert_eq!(code, ServerErrorCode::AgentDead);
            assert!(message.contains("死亡"));
            assert!(tick_id.is_none()); // 消息中没有 tick_id
            assert_eq!(current_tick, Some(100)); // 传入的 current_tick
        }
        _ => panic!("Expected ServerError"),
    }
}

#[test]
fn test_from_server_message_error_with_tick() {
    let server_msg = ServerMessage::Error {
        code: String::new(),
        message: "tick 105: invalid action".to_string(),
        current_tick_id: None,
    };

    let result = DownstreamMessage::from_server_message(server_msg, 100);
    assert!(result.is_some());

    match result.unwrap() {
        DownstreamMessage::ServerError {
            code: _,
            message,
            tick_id,
            current_tick,
        } => {
            assert_eq!(tick_id, Some(105));
            assert!(message.contains("tick 105"));
            assert_eq!(current_tick, Some(100)); // 传入的 current_tick
        }
        _ => panic!("Expected ServerError"),
    }
}

#[test]
fn test_from_server_message_dialogue_request() {
    let from_id = Uuid::new_v4();
    let to_id = Uuid::new_v4();

    let server_msg = ServerMessage::Dialogue {
        message: DialogueMessage::Request {
            from_agent_id: from_id,
            to_agent_id: to_id,
            opening_remark: "少侠，可否借一步说话？".to_string(),
        },
    };

    let result = DownstreamMessage::from_server_message(server_msg, 100);
    assert!(result.is_some());

    match result.unwrap() {
        DownstreamMessage::ServerDialogue {
            dialogue_type,
            from_agent_id,
            to_agent_id,
            session_id,
            opening_remark,
            content,
        } => {
            assert_eq!(dialogue_type, "request");
            assert_eq!(from_agent_id, from_id);
            assert_eq!(to_agent_id, Some(to_id));
            assert!(session_id.is_none());
            assert_eq!(opening_remark, Some("少侠，可否借一步说话？".to_string()));
            assert!(content.is_none());
        }
        _ => panic!("Expected ServerDialogue"),
    }
}

#[test]
fn test_from_server_message_dialogue_content() {
    let from_id = Uuid::new_v4();

    let server_msg = ServerMessage::Dialogue {
        message: DialogueMessage::Content {
            from_agent_id: from_id,
            session_id: "session-123".to_string(),
            content: "今天天气不错。".to_string(),
        },
    };

    let result = DownstreamMessage::from_server_message(server_msg, 100);
    assert!(result.is_some());

    match result.unwrap() {
        DownstreamMessage::ServerDialogue {
            dialogue_type,
            from_agent_id,
            to_agent_id,
            session_id,
            opening_remark,
            content,
        } => {
            assert_eq!(dialogue_type, "content");
            assert_eq!(from_agent_id, from_id);
            assert!(to_agent_id.is_none());
            assert_eq!(session_id, Some("session-123".to_string()));
            assert!(opening_remark.is_none());
            assert_eq!(content, Some("今天天气不错。".to_string()));
        }
        _ => panic!("Expected ServerDialogue"),
    }
}

#[test]
fn test_from_server_message_game_rules_update() {
    let server_msg = ServerMessage::ConfigUpdate {
        config_type: cyber_jianghu_protocol::ConfigType::GameRules,
        update_type: "full".to_string(),
        version: "0.0.6".to_string(),
        content: serde_json::json!({
            "tick_duration_secs": 30,
            "available_actions": [],
            "initial_items": [],
            "survival_actions": [],
            "rebirth_delay_ticks": 0,
            "version": "0.0.6",
            "last_updated": "2024-03-22T12:00:00Z",
            "rebirth_retry_max_attempts": 3,
            "rebirth_retry_interval_secs": 30,
            "intent_batch": null,
            "immediate_events": null,
        }),
        content_hash: None,
        updated_items: vec![],
        removed_items: vec![],
    };

    let result = DownstreamMessage::from_server_message(server_msg, 100);
    assert!(result.is_some());

    match result.unwrap() {
        DownstreamMessage::ServerGameRulesUpdate {
            tick_duration_secs,
            version,
            last_updated,
        } => {
            assert_eq!(tick_duration_secs, 30);
            assert_eq!(version, "0.0.6");
            assert_eq!(last_updated, "2024-03-22T12:00:00Z");
        }
        _ => panic!("Expected ServerGameRulesUpdate"),
    }
}

#[test]
fn test_from_server_message_world_state_skipped() {
    // WorldState 不应该被转换（已有专门的 Tick 处理）
    let server_msg = ServerMessage::WorldState {
        data: create_test_world_state(),
    };

    let result = DownstreamMessage::from_server_message(server_msg, 100);
    assert!(result.is_none());
}

#[test]
fn test_deserialize_intent_without_subsequent_is_backward_compatible() {
    // 旧上游不发送 subsequent_intents 字段 → serde default 空 Vec（wire 兼容）
    let json = r#"{"type":"intent","tick_id":1,"action_type":"休整"}"#;
    let msg: UpstreamMessage = serde_json::from_str(json).unwrap();
    match msg {
        UpstreamMessage::Intent {
            subsequent_intents, ..
        } => assert!(subsequent_intents.is_empty()),
        _ => panic!("Expected Intent message"),
    }
}

#[test]
fn test_deserialize_intent_with_subsequent_queue() {
    let json = r#"{"type":"intent","tick_id":1,"action_type":"移动","action_data":{"target":"市场"},"subsequent_intents":[{"type":"intent","action_type":"说话","action_data":{"content":"到了"}}]}"#;
    let msg: UpstreamMessage = serde_json::from_str(json).unwrap();
    match msg {
        UpstreamMessage::Intent {
            action_type,
            subsequent_intents,
            ..
        } => {
            assert_eq!(action_type, "移动");
            assert_eq!(subsequent_intents.len(), 1);
            assert_eq!(subsequent_intents[0].action_type, "说话");
        }
        _ => panic!("Expected Intent message"),
    }
}
