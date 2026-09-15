//! client 模块单测（trait 契约 + JSON 提取/修复）

use super::json_utils::{find_first_json_end, repair_llm_json};
use super::mock::*;
use super::*;

#[tokio::test]
async fn test_mock_llm_client_complete() {
    let client = MockLlmClient::with_response("Hello, world!");
    let result = client.complete("test prompt").await.unwrap();
    assert_eq!(result, "Hello, world!");
}

#[tokio::test]
async fn test_mock_llm_client_complete_json() {
    #[derive(serde::Deserialize)]
    struct TestResponse {
        message: String,
    }

    let client = MockLlmClient::with_response(r#"{"message": "test"}"#);
    let result: TestResponse = client.complete_json("test prompt").await.unwrap();
    assert_eq!(result.message, "test");
}

#[test]
fn parse_json_response_malformed_multibyte_returns_err_without_panic() {
    // 诊断路径（error_snippet + 错误详情直写 message）对含中文的
    // 非法 JSON 不得 panic，且必须返回 Err 供上层重试
    let malformed = r#"{"action": "抱拳行礼", "target": 损坏的引号"#;
    let result = parse_json_response::<serde_json::Value>(malformed);
    assert!(result.is_err());

    // 错误行号定位涉及多字节内容时同样安全
    let malformed2 = format!("{}\n{}", "汉字行".repeat(50), r#"{"action": }"#);
    assert!(parse_json_response::<serde_json::Value>(&malformed2).is_err());
}

// ========================================================================
// find_first_json_end tests
// ========================================================================

#[test]
fn test_find_json_simple() {
    let s = r#"{"a":1}"#;
    assert_eq!(find_first_json_end(s), Some(6));
}

#[test]
fn test_find_json_with_trailing() {
    let s = r#"{"a":1}{"b":2}"#;
    assert_eq!(find_first_json_end(s), Some(6));
}

#[test]
fn test_find_json_nested() {
    let s = r#"{"a":{"b":2}}"#;
    assert_eq!(find_first_json_end(s), Some(12));
}

#[test]
fn test_find_json_string_with_braces() {
    let s = r#"{"a":"{b}"}"#;
    assert_eq!(find_first_json_end(s), Some(10));
}

#[test]
fn test_find_json_escaped_quotes() {
    let s = r#"{"a":"he said \"hello\""}"#;
    assert_eq!(find_first_json_end(s), Some(24));
}

#[test]
fn test_find_json_no_object() {
    let s = "no json here";
    assert_eq!(find_first_json_end(s), None);
}

#[test]
fn test_find_json_with_prefix() {
    let s = r#"some text {"a":1}"#;
    assert_eq!(find_first_json_end(s), Some(16));
}

// ========================================================================
// repair_llm_json tests
// ========================================================================

#[test]
fn test_repair_valid_json_passthrough() {
    let input = r#"{"name": "test", "age": 25}"#;
    let result = repair_llm_json(input);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["name"], "test");
    assert_eq!(parsed["age"], 25);
}

#[test]
fn test_repair_embedded_unescaped_quotes() {
    // 实际 LongCat-2.0-Preview 输出：identity 值内含未转义 ASCII "
    let input = r#"{"identity": "曾是江湖上赫赫有名的"追魂针"沈三"}"#;
    let result = repair_llm_json(input);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(parsed["identity"].as_str().unwrap().contains("追魂针"));
    assert!(parsed["identity"].as_str().unwrap().contains("沈三"));
}

#[test]
fn test_repair_embedded_quotes_before_comma() {
    let input = r#"{"desc": "他说"完毕"后离开", "name": "test"}"#;
    let result = repair_llm_json(input);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(parsed["desc"].as_str().unwrap().contains("完毕"));
    assert_eq!(parsed["name"], "test");
}

#[test]
fn test_repair_chinese_quotes_as_delimiters() {
    let input = "{\u{201c}name\u{201d}: \u{201c}test\u{201d}}";
    let result = repair_llm_json(input);
    assert!(
        serde_json::from_str::<serde_json::Value>(&result).is_ok(),
        "Chinese quotes should produce valid JSON, got: {}",
        result
    );
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["name"], "test");
}

#[test]
fn test_repair_unbalanced_brackets() {
    let input = r#"{"a": {"b": 1"#;
    let result = repair_llm_json(input);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["a"]["b"], 1);
}

#[test]
fn test_repair_trailing_comma() {
    let input = r#"{"a": 1,}"#;
    let result = repair_llm_json(input);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["a"], 1);
}

#[test]
fn test_repair_line_comment() {
    let input = "{\n  \"a\": 1 // comment\n}";
    let result = repair_llm_json(input);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["a"], 1);
}

#[test]
fn test_repair_string_at_end_of_input() {
    // 字符串值在输入末尾闭合，next_meaningful = None
    let input = r#"{"a": "test"}"#;
    let result = repair_llm_json(input);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["a"], "test");
}

#[test]
fn test_repair_unclosed_string() {
    let input = r#"{"a": "test"#;
    let result = repair_llm_json(input);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["a"], "test");
}

#[test]
fn test_repair_quote_before_colon_in_value() {
    // " 后跟 : 不应被误判为闭合（已从匹配集中移除 :）
    let input = r#"{"desc": "a:b"}"#;
    let result = repair_llm_json(input);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["desc"], "a:b");
}

// ========================================================================
// SharedBreaker tests — 验证 disable / is_disabled / 自动清理过期项
// ========================================================================

#[test]
fn test_shared_breaker_disable_and_query() {
    let breaker = SharedBreaker::new();
    // 初始：所有 key 都可用
    assert!(
        breaker
            .is_disabled("openai_compatible/sensenova-6.7-flash-lite")
            .is_none()
    );

    // 禁用后：返回剩余秒数
    breaker.disable(
        "openai_compatible/sensenova-6.7-flash-lite".to_string(),
        3600,
    );
    let remaining = breaker.is_disabled("openai_compatible/sensenova-6.7-flash-lite");
    assert!(remaining.is_some(), "禁用后应返回 Some(remaining)");
    let secs = remaining.unwrap();
    assert!(secs > 0 && secs <= 3600, "剩余秒数应在 (0, 3600] 区间");

    // 其他 key 不受影响
    assert!(
        breaker
            .is_disabled("openai_compatible/other-model")
            .is_none()
    );
}

#[test]
fn test_shared_breaker_overwrite_disable() {
    let breaker = SharedBreaker::new();
    let key = "openai_compatible/x".to_string();
    breaker.disable(key.clone(), 60);
    // 二次 disable 不应 panic
    breaker.disable(key, 60);
    assert!(breaker.is_disabled("openai_compatible/x").is_some());
}

#[test]
fn test_shared_breaker_default() {
    let breaker = SharedBreaker::default();
    assert!(breaker.is_disabled("any/key").is_none());
}

#[test]
fn test_shared_breaker_is_send_sync() {
    // 编译期断言：SharedBreaker 必须能跨线程共享
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SharedBreaker>();
}

#[test]
fn build_conversation_messages_strips_reasoning_when_flag_set() {
    let turns = vec![ConversationTurn {
        user: "user".to_string(),
        assistant: "reply".to_string(),
        reasoning_content: Some("reasoning to strip".to_string()),
    }];
    let messages = build_conversation_messages("sys", "", None, &turns, "current", true);
    let assistant_msg = messages.iter().find(|m| m.role == "assistant").unwrap();
    let json = serde_json::to_value(assistant_msg).unwrap();
    assert!(
        json.get("reasoning_content").is_none() || json["reasoning_content"].is_null(),
        "reasoning_content should be None when strip_reasoning=true, got: {:?}",
        json
    );
}

#[test]
fn build_conversation_messages_preserves_reasoning_when_flag_unset() {
    let turns = vec![ConversationTurn {
        user: "u".to_string(),
        assistant: "a".to_string(),
        reasoning_content: Some("reasoning".to_string()),
    }];
    let messages = build_conversation_messages("sys", "", None, &turns, "current", false);
    let assistant_msg = messages.iter().find(|m| m.role == "assistant").unwrap();
    let json = serde_json::to_value(assistant_msg).unwrap();
    assert_eq!(json["reasoning_content"], "reasoning");
}
