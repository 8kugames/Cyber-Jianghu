//! llm_config 模块单测（自 llm_config.rs 外移，内容未改）

use super::resolve_api_key;
use crate::component::llm::LlmProvider;
use crate::component::llm::token_tracking::ModelTokenStats;
use std::collections::HashMap;

#[test]
fn metrics_query_filters_by_system_hash() {
    let mut stats = vec![
        ModelTokenStats {
            provider: LlmProvider::OpenAICompatible.as_str().to_string(),
            model: "model-A".to_string(),
            system_hash_distribution: {
                let mut m = HashMap::new();
                m.insert([1u8; 32], 5);
                m.insert([2u8; 32], 3);
                m
            },
            ..Default::default()
        },
        ModelTokenStats {
            provider: LlmProvider::OpenAICompatible.as_str().to_string(),
            model: "model-B".to_string(),
            system_hash_distribution: {
                let mut m = HashMap::new();
                m.insert([3u8; 32], 7);
                m
            },
            ..Default::default()
        },
    ];

    assert_eq!(stats.len(), 2);

    let target = [1u8; 32];
    stats.retain(|s| s.system_hash_distribution.contains_key(&target));
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].model, "model-A");
}

#[test]
fn metrics_query_hex_decode_32_bytes() {
    let hex_str = "0101010101010101010101010101010101010101010101010101010101010101";
    let bytes = hex::decode(hex_str).unwrap();
    assert_eq!(bytes.len(), 32);
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    assert_eq!(arr, [1u8; 32]);
}

#[test]
fn resolve_api_key_uses_request_when_non_empty() {
    // 用户重新输入密钥 → 用新值
    assert_eq!(resolve_api_key("sk-new", Some("sk-old")), "sk-new");
}

#[test]
fn resolve_api_key_trims_request_value() {
    // 前端 trim 后发送，后端再次 trim 保持幂等
    assert_eq!(resolve_api_key("  sk-new  ", Some("sk-old")), "sk-new");
}

#[test]
fn resolve_api_key_falls_back_to_saved_when_request_empty() {
    // 用户未修改密钥（空串）→ 复用已保存值（401 missing_api_key 根因修复）
    assert_eq!(resolve_api_key("", Some("sk-saved")), "sk-saved");
}

#[test]
fn resolve_api_key_falls_back_to_saved_when_request_blank() {
    // 纯空白也视为"未修改"
    assert_eq!(resolve_api_key("   ", Some("sk-saved")), "sk-saved");
}

#[test]
fn resolve_api_key_trims_saved_value() {
    assert_eq!(resolve_api_key("", Some("  sk-saved  ")), "sk-saved");
}

#[test]
fn resolve_api_key_returns_empty_when_both_empty() {
    // 两者皆空 → 空串（下游既有"空串→None"语义保持不变）
    assert_eq!(resolve_api_key("", None), "");
}

#[test]
fn resolve_api_key_returns_empty_when_saved_is_blank() {
    // 已保存值为空串/纯空白 → 视为无密钥
    assert_eq!(resolve_api_key("", Some("")), "");
    assert_eq!(resolve_api_key("", Some("   ")), "");
}

#[test]
fn resolve_api_key_request_takes_precedence_over_blank_saved() {
    // 即使 saved 为空，req 非空仍用 req
    assert_eq!(resolve_api_key("sk-new", Some("")), "sk-new");
}
