//! sft_transform 纯函数测试
//!
//! 对照基准: scripts/build_sft_data.py --no-db-filter 模式 (跳过天魂筛选, 仍执行
//! ok 过滤 + persona 条件 append). 天魂筛选逻辑用独立 fixture 验证.

use cyber_jianghu_protocol::TraceEntry;
use cyber_jianghu_server::training_export::sft_transform::{TransformInput, transform_entry};

fn make_trace(ok: bool, response: &str, persona_name: &str, persona_desc: &str) -> TraceEntry {
    TraceEntry {
        trace_id: "test-trace-001".to_string(),
        agent_id: uuid::Uuid::nil(),
        character_name: "TestAgent".to_string(),
        tick_id: 42,
        soul_stage: "Renhun".to_string(),
        attempt: 0,
        provider: "test".to_string(),
        model: "test-model".to_string(),
        persona_name: persona_name.to_string(),
        persona_description: persona_desc.to_string(),
        user_prompt: "用户提示".to_string(),
        response: response.to_string(),
        prompt_tokens: None,
        completion_tokens: None,
        ok,
        wall_clock: None,
    }
}

#[test]
fn test_ok_false_skipped() {
    let trace = make_trace(false, "有 response 但 ok=false", "张三", "描述");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    assert_eq!(transform_entry(input), None, "ok=false 必须跳过");
}

#[test]
fn test_empty_response_skipped() {
    let trace = make_trace(true, "   ", "张三", "描述");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    assert_eq!(transform_entry(input), None, "空 response 必须跳过");
}

#[test]
fn test_persona_name_with_description() {
    let trace = make_trace(true, "回复内容", "张三", "是个侠客");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    let sample = transform_entry(input).expect("应产出样本");
    assert_eq!(sample.messages.len(), 3);
    assert_eq!(sample.messages[0].role, "system");
    assert_eq!(sample.messages[0].content, "你是 张三。\n是个侠客");
    assert_eq!(sample.messages[1].role, "user");
    assert_eq!(sample.messages[1].content, "用户提示");
    assert_eq!(sample.messages[2].role, "assistant");
    assert_eq!(sample.messages[2].content, "回复内容");
}

#[test]
fn test_persona_name_without_description() {
    let trace = make_trace(true, "回复", "李四", "");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    let sample = transform_entry(input).expect("应产出样本");
    assert_eq!(sample.messages.len(), 3);
    assert_eq!(sample.messages[0].content, "你是 李四。");
}

#[test]
fn test_persona_empty_no_system_message() {
    let trace = make_trace(true, "回复", "", "有描述但无名字");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    let sample = transform_entry(input).expect("应产出样本");
    assert_eq!(sample.messages.len(), 2, "persona_name 空时无 system");
    assert_eq!(sample.messages[0].role, "user");
    assert_eq!(sample.messages[1].role, "assistant");
}

#[test]
fn test_persona_both_empty_still_exported() {
    let trace = make_trace(true, "回复", "", "");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: None,
    };
    let sample = transform_entry(input).expect("双空也导出");
    assert_eq!(sample.messages.len(), 2);
}

#[test]
fn test_metadata_fields_populated() {
    let trace = make_trace(true, "回复", "张三", "描述");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    let sample = transform_entry(input).expect("应产出样本");
    let meta = &sample.metadata;
    assert_eq!(meta.agent_id, "00000000-0000-0000-0000-000000000000");
    assert_eq!(meta.tick_id, 42);
    assert_eq!(meta.soul_stage, "Renhun");
    assert_eq!(meta.attempt, 0);
    assert_eq!(meta.tianhun_result, Some("approved".to_string()));
    assert_eq!(meta.trace_id, "test-trace-001");
}

// ---- 历史动作名归一（LEGACY_ACTION_ALIASES / TOOL_TRACE_ACTION_TYPES）----

fn assistant_content(
    sample: &cyber_jianghu_server::training_export::sft_transform::SftSample,
) -> &str {
    sample
        .messages
        .iter()
        .find(|m| m.role == "assistant")
        .map(|m| m.content.as_str())
        .expect("应有 assistant 消息")
}

#[test]
fn test_legacy_action_names_normalized() {
    let response = r#"{"actions":[{"action_type":"进食","action_data":{"item_id":"a"}},{"action_type":"打坐","action_data":{}},{"action_type":"说话","action_data":{"content":"hi"}}]}"#;
    let trace = make_trace(true, response, "张三", "");
    let sample = transform_entry(TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    })
    .expect("应产出样本");
    let parsed: serde_json::Value =
        serde_json::from_str(assistant_content(&sample)).expect("归一后仍为合法 JSON");
    let types: Vec<&str> = parsed["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action_type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        vec!["吃", "休整", "说话"],
        "旧动作名必须归一为 v2.0 名"
    );
    assert_eq!(
        parsed["actions"][0]["action_data"]["item_id"], "a",
        "action_data 原样保留"
    );
}

#[test]
fn test_tool_trace_actions_dropped() {
    let response = r#"{"actions":[{"action_type":"query_world","action_data":{}},{"action_type":"观察","action_data":{}}]}"#;
    let trace = make_trace(true, response, "张三", "");
    let sample = transform_entry(TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    })
    .expect("应产出样本");
    let parsed: serde_json::Value = serde_json::from_str(assistant_content(&sample)).unwrap();
    let types: Vec<&str> = parsed["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action_type"].as_str().unwrap())
        .collect();
    assert_eq!(types, vec!["观察"], "工具残留丢弃，正常动作保留");
}

#[test]
fn test_tool_only_sample_dropped() {
    let response = r#"{"actions":[{"action_type":"查询状态","action_data":{}}]}"#;
    let trace = make_trace(true, response, "张三", "");
    assert_eq!(
        transform_entry(TransformInput {
            entry: &trace,
            tianhun_result: Some("approved".to_string()),
        }),
        None,
        "全部动作是工具残留时丢弃整样本"
    );
}

#[test]
fn test_non_json_response_passthrough() {
    let trace = make_trace(true, "  出门练剑。  ", "张三", "");
    let sample = transform_entry(TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    })
    .expect("纯文本 response 是合法导出形态");
    assert_eq!(
        assistant_content(&sample),
        "出门练剑。",
        "非 JSON response 原样透传（含前置 trim）"
    );
}

#[test]
fn test_json_without_legacy_actions_semantics_unchanged() {
    let response = r#"{"actions":[{"action_type":"说话","action_data":{"content":"hi"}}],"thought_process":"思考"}"#;
    let trace = make_trace(true, response, "张三", "");
    let sample = transform_entry(TransformInput {
        entry: &trace,
        tianhun_result: None,
    })
    .expect("应产出样本");
    let parsed: serde_json::Value = serde_json::from_str(assistant_content(&sample)).unwrap();
    let expected: serde_json::Value = serde_json::from_str(response).unwrap();
    assert_eq!(
        parsed, expected,
        "无旧动作名的 JSON 语义不变（允许字段重排）"
    );
}
