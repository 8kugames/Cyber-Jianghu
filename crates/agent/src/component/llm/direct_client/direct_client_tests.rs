//! direct_client 模块单测（构造/请求/streaming/breaker 集成）

use super::*;

#[test]
fn test_provider_from_str() {
    assert_eq!(LlmProvider::parse("openclaw"), Some(LlmProvider::OpenClaw));
    assert_eq!(LlmProvider::parse("OpenClaw"), Some(LlmProvider::OpenClaw));
    assert_eq!(
        LlmProvider::parse("openai_compatible"),
        Some(LlmProvider::OpenAICompatible)
    );
    assert_eq!(
        LlmProvider::parse("openai-compatible"),
        Some(LlmProvider::OpenAICompatible)
    );
    assert_eq!(LlmProvider::parse("ollama"), Some(LlmProvider::Ollama));
    assert_eq!(LlmProvider::parse("unknown"), None);
}

#[test]
fn utf8_safe_end_never_lands_inside_multibyte_char() {
    // 回归：direct_client.rs tool_calls 预览切片曾在 '人'(3 字节) 内部切片 panic
    let s = format!("{}\"tool_calls\"{}", "汉".repeat(1000), "人".repeat(1000));
    let tc_start = s.find("\"tool_calls\"").unwrap();
    let end = utf8_safe_end(&s, tc_start + 3000);
    assert!(s.is_char_boundary(end));
    assert!(end >= tc_start && end <= s.len());
    // 切片不再 panic
    let _preview = &s[tc_start..end];

    // 边界：超出长度回退到 len；0 与空串安全
    assert_eq!(utf8_safe_end(s.as_str(), usize::MAX), s.len());
    assert_eq!(utf8_safe_end("", 10), 0);
    // 单字符内部偏移回退到该字符起点
    assert_eq!(utf8_safe_end("人", 1), 0);
    assert_eq!(utf8_safe_end("人", 2), 0);
    assert_eq!(utf8_safe_end("人", 3), 3);
}

#[test]
fn track_system_hash_warns_only_for_new_hashes() {
    let mut known = std::collections::HashSet::new();
    let actor = [1u8; 32];
    let validator = [2u8; 32];

    assert!(track_system_hash(&mut known, actor), "首次出现应告警");
    assert!(
        track_system_hash(&mut known, validator),
        "第二种合法 prompt 首次出现应告警"
    );
    // actor/validator 交替不再误报
    for _ in 0..10 {
        assert!(!track_system_hash(&mut known, actor));
        assert!(!track_system_hash(&mut known, validator));
    }
    assert!(track_system_hash(&mut known, [3u8; 32]), "全新 hash 应告警");
}

#[test]
fn track_system_hash_resets_at_capacity() {
    let mut known = std::collections::HashSet::new();
    for i in 0..KNOWN_SYSTEM_HASHES_MAX {
        let mut h = [0u8; 32];
        h[0] = i as u8;
        track_system_hash(&mut known, h);
    }
    assert_eq!(known.len(), KNOWN_SYSTEM_HASHES_MAX);
    // 超限后重置：集合被清空，当前 hash 重新视为新 hash
    let mut overflow = [9u8; 32];
    overflow[1] = 1;
    assert!(track_system_hash(&mut known, overflow));
    assert_eq!(known.len(), 1);
}

#[test]
fn test_provider_defaults() {
    // OpenClaw 从配置文件读取 base_url/model，但需要用户输入 API Key
    assert_eq!(LlmProvider::OpenClaw.default_base_url(), None);
    assert_eq!(LlmProvider::OpenClaw.default_model(), None);
    assert!(LlmProvider::OpenClaw.requires_api_key()); // 用户需要手动输入
    assert!(!LlmProvider::OpenClaw.requires_base_url()); // 从配置文件读取
    assert!(!LlmProvider::OpenClaw.requires_model()); // 从配置文件读取
    assert!(LlmProvider::OpenClaw.reads_from_config());

    // OpenAICompatible 没有默认值
    assert_eq!(LlmProvider::OpenAICompatible.default_base_url(), None);
    assert_eq!(LlmProvider::OpenAICompatible.default_model(), None);
    assert!(LlmProvider::OpenAICompatible.requires_api_key());
    assert!(LlmProvider::OpenAICompatible.requires_base_url());
    assert!(LlmProvider::OpenAICompatible.requires_model());

    // Ollama 有默认 URL 但没有默认模型
    assert_eq!(
        LlmProvider::Ollama.default_base_url(),
        Some("http://localhost:11434/v1")
    );
    assert_eq!(LlmProvider::Ollama.default_model(), None);
    assert!(!LlmProvider::Ollama.requires_api_key());
    assert!(!LlmProvider::Ollama.requires_base_url());
    assert!(!LlmProvider::Ollama.requires_model());
}

/// 验证：DirectLlmClientConfig 默认 120s/30s timeout，并提供
/// `with_request_timeout_secs` / `with_connect_timeout_secs` builder。
/// 修复前 `build_http_client` 硬编码 120s，agent 端 `cognitive_decision_with_chain`
/// 最坏耗时 = 12 retries × 120s = 24min，无外部闸门。
#[test]
fn test_p1_f6_config_default_timeouts_and_builder() {
    let config = DirectLlmClientConfig::new(LlmProvider::Ollama, None::<String>);
    assert_eq!(
        config.request_timeout_secs, 120,
        "默认 request_timeout_secs 必须 120s（与 Server LlmConfig 对齐）"
    );
    assert_eq!(
        config.connect_timeout_secs, 30,
        "默认 connect_timeout_secs 必须 30s（与 Server LlmConfig 对齐）"
    );

    let config = DirectLlmClientConfig::new(LlmProvider::Ollama, None::<String>)
        .with_request_timeout_secs(60)
        .with_connect_timeout_secs(15);
    assert_eq!(config.request_timeout_secs, 60);
    assert_eq!(config.connect_timeout_secs, 15);
}

#[test]
fn test_config_builder() {
    let config = DirectLlmClientConfig::new(LlmProvider::OpenClaw, Some("test-key"))
        .with_model("custom-model")
        .with_temperature(0.5)
        .with_max_tokens(2048);

    assert_eq!(config.provider, LlmProvider::OpenClaw);
    assert_eq!(config.api_key, Some("test-key".to_string()));
    assert_eq!(config.model, Some("custom-model".to_string()));
    assert_eq!(config.temperature, 0.5);
    assert_eq!(config.max_tokens, 2048);
}

#[test]
fn test_config_validate() {
    // OpenAICompatible 需要 base_url 和 model
    let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"));
    assert!(config.validate().is_err());

    let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"))
        .with_base_url("https://api.example.com");
    assert!(config.validate().is_err()); // 仍然缺少 model

    let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"))
        .with_base_url("https://api.example.com")
        .with_model("gpt-4");
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_get_base_url() {
    // Ollama 默认 URL
    let config = DirectLlmClientConfig::new(LlmProvider::Ollama, None::<String>);
    assert_eq!(config.get_base_url().unwrap(), "http://localhost:11434/v1");

    // 覆盖默认 URL
    let config = config.with_base_url("https://custom.api/v1");
    assert_eq!(config.get_base_url().unwrap(), "https://custom.api/v1");

    // OpenAICompatible 没有默认 URL
    let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"))
        .with_model("gpt-4");
    assert!(config.get_base_url().is_err());

    let config = config.with_base_url("https://api.example.com");
    assert_eq!(config.get_base_url().unwrap(), "https://api.example.com");
}

#[test]
fn test_config_get_model() {
    // OpenClaw 返回默认值
    let config = DirectLlmClientConfig::new(LlmProvider::OpenClaw, None::<String>);
    assert_eq!(config.get_model_with_default(), "default");

    // 覆盖模型
    let config = config.with_model("custom-model");
    assert_eq!(config.get_model_with_default(), "custom-model");

    // Ollama 没有默认模型
    let config = DirectLlmClientConfig::new(LlmProvider::Ollama, None::<String>);
    assert_eq!(config.get_model_with_default(), "default");

    // OpenAICompatible 没有默认模型
    let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"))
        .with_base_url("https://api.example.com");
    assert_eq!(config.get_model_with_default(), "default");

    let config = config.with_model("gpt-4");
    assert_eq!(config.get_model_with_default(), "gpt-4");
}

#[test]
fn test_direct_client_openclaw() {
    // OpenClaw 不需要 API key，从配置文件读取
    let config = DirectLlmClientConfig::new(LlmProvider::OpenClaw, None::<String>);
    assert_eq!(config.provider, LlmProvider::OpenClaw);
    assert_eq!(config.api_key, None);
    assert_eq!(config.base_url, None);
}

#[test]
fn test_direct_client_openai_compatible_missing_fields() {
    // 缺少 base_url 和 model
    assert!(
        DirectLlmClient::new(DirectLlmClientConfig::new(
            LlmProvider::OpenAICompatible,
            Some("test-key")
        ))
        .is_err()
    );
}

#[test]
fn test_direct_client_ollama() {
    let client = DirectLlmClient::ollama(None::<String>).unwrap();
    assert_eq!(client.config.provider, LlmProvider::Ollama);
    assert_eq!(client.config.api_key, None);
    assert_eq!(client.config.base_url, None); // 使用默认

    let client = DirectLlmClient::ollama(Some("http://localhost:11434/v1")).unwrap();
    assert_eq!(
        client.config.base_url,
        Some("http://localhost:11434/v1".to_string())
    );
}

#[test]
fn test_temperature_clamping() {
    let config =
        DirectLlmClientConfig::new(LlmProvider::OpenClaw, Some("test-key")).with_temperature(-0.5);
    assert_eq!(config.temperature, 0.0);

    let config =
        DirectLlmClientConfig::new(LlmProvider::OpenClaw, Some("test-key")).with_temperature(1.5);
    assert_eq!(config.temperature, 1.0);
}

// ========================================================================
// SharedBreaker 集成测试 — 验证 check_breaker / breaker_key / 注入流程
// ========================================================================

use super::super::client::SharedBreaker;
use std::sync::Arc;

fn make_test_client_with_breaker() -> (DirectLlmClient, Arc<SharedBreaker>) {
    let breaker = Arc::new(SharedBreaker::new());
    let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"))
        .with_base_url("https://example.com/v1")
        .with_model("test-model");
    let client = DirectLlmClient::new(config)
        .unwrap()
        .with_breaker(breaker.clone());
    (client, breaker)
}

#[test]
fn test_breaker_key_format() {
    let (client, _) = make_test_client_with_breaker();
    assert_eq!(client.breaker_key(), "openai_compatible/test-model");
}

#[test]
fn test_check_breaker_allows_when_not_disabled() {
    let (client, _) = make_test_client_with_breaker();
    // 初始状态：breaker 表为空 → check_breaker 通过
    assert!(client.check_breaker().is_ok());
}

#[test]
fn test_check_breaker_rejects_when_disabled() {
    let (client, breaker) = make_test_client_with_breaker();
    // 禁用该 client 对应的 key
    let key = client.breaker_key();
    breaker.disable(key.clone(), 60);

    // check_breaker 必须返回 Err，且错误信息包含 "cooldown"
    let result = client.check_breaker();
    assert!(result.is_err(), "禁用后 check_breaker 应返回 Err");
    let err_msg = format!("{:#}", result.unwrap_err());
    assert!(
        err_msg.contains("cooldown"),
        "错误信息应提示冷却，实际: {}",
        err_msg
    );
    assert!(
        err_msg.contains(&key),
        "错误信息应包含 key，实际: {}",
        err_msg
    );
}

#[test]
fn test_check_breaker_independent_keys() {
    // 验证：breaker key 隔离 — 禁用 model A 不影响 model B
    let breaker = Arc::new(SharedBreaker::new());
    let config_a = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("k"))
        .with_base_url("https://example.com/v1")
        .with_model("model-a");
    let client_a = DirectLlmClient::new(config_a)
        .unwrap()
        .with_breaker(breaker.clone());
    let config_b = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("k"))
        .with_base_url("https://example.com/v1")
        .with_model("model-b");
    let client_b = DirectLlmClient::new(config_b)
        .unwrap()
        .with_breaker(breaker.clone());

    // 禁用 model-a
    breaker.disable(client_a.breaker_key(), 60);

    // model-a 被拒，model-b 仍可用
    assert!(client_a.check_breaker().is_err());
    assert!(client_b.check_breaker().is_ok());
}

#[test]
fn test_breaker_clone_shares_state() {
    // 验证：Clone 后的 DirectLlmClient 与原 client 共享同一份 breaker
    let (client, breaker) = make_test_client_with_breaker();
    let cloned = client.clone();

    // 在原 breaker 上禁用
    breaker.disable(client.breaker_key(), 60);

    // 克隆体也应命中
    assert!(cloned.check_breaker().is_err());
}
