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
    assert_eq!(meta.tianhun_result, "approved");
    assert_eq!(meta.trace_id, "test-trace-001");
}
