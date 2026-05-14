//! Per-component token metrics 单元测试
//!
//! 验证 LlmComponent + ComponentMetrics 的记录和快照功能。

use cyber_jianghu_agent::component::llm::{
    ComponentMetrics, LlmComponent, LlmProvider,
    record_token_usage_with_component, snapshot_component_stats,
};

/// 辅助：解析 provider（测试用）
fn test_provider() -> LlmProvider {
    LlmProvider::parse("openai_compatible").unwrap()
}

#[test]
fn component_metrics_record_increments_counts() {
    // snapshot 获取当前状态（不依赖顺序，只验证结构正确）
    let provider = test_provider();
    let component = LlmComponent::CognitiveEngine;

    record_token_usage_with_component(
        &provider,
        "test-model",
        100,
        50,
        component.clone(),
    );

    let stats = snapshot_component_stats();
    let metrics = stats.get(&component).expect("should have CognitiveEngine entry");
    assert_eq!(metrics.call_count, 1);
    assert_eq!(metrics.total_input_tokens, 100);
    assert_eq!(metrics.total_output_tokens, 50);
}

#[test]
fn snapshot_component_stats_returns_multiple_components() {
    let provider = test_provider();

    record_token_usage_with_component(
        &provider,
        "test-model",
        200,
        80,
        LlmComponent::ReflectorLayer3,
    );
    record_token_usage_with_component(
        &provider,
        "test-model",
        150,
        40,
        LlmComponent::SocialProcessing,
    );

    let stats = snapshot_component_stats();

    let reflector = stats.get(&LlmComponent::ReflectorLayer3).expect("ReflectorLayer3");
    assert_eq!(reflector.call_count, 1);
    assert_eq!(reflector.total_input_tokens, 200);
    assert_eq!(reflector.total_output_tokens, 80);

    let social = stats.get(&LlmComponent::SocialProcessing).expect("SocialProcessing");
    assert_eq!(social.call_count, 1);
    assert_eq!(social.total_input_tokens, 150);
    assert_eq!(social.total_output_tokens, 40);
}

#[test]
fn multiple_calls_accumulate_per_component() {
    let provider = test_provider();
    let component = LlmComponent::ToolCalling;

    record_token_usage_with_component(&provider, "m1", 100, 50, component.clone());
    record_token_usage_with_component(&provider, "m2", 200, 100, component.clone());
    record_token_usage_with_component(&provider, "m3", 300, 150, component.clone());

    let stats = snapshot_component_stats();
    let metrics = stats.get(&component).expect("ToolCalling");
    assert_eq!(metrics.call_count, 3);
    assert_eq!(metrics.total_input_tokens, 600);
    assert_eq!(metrics.total_output_tokens, 300);
}

#[test]
fn component_metrics_default_is_zero() {
    let metrics = ComponentMetrics::default();
    assert_eq!(metrics.call_count, 0);
    assert_eq!(metrics.total_input_tokens, 0);
    assert_eq!(metrics.total_output_tokens, 0);
}

#[test]
fn llm_component_equality_and_hash() {
    // 验证 enum 的 PartialEq + Hash 派生正常工作
    let a = LlmComponent::CognitiveEngine;
    let b = LlmComponent::CognitiveEngine;
    let c = LlmComponent::AttentionController;

    assert_eq!(a, b);
    assert_ne!(a, c);

    // 能用作 HashMap key
    let mut map = std::collections::HashMap::new();
    map.insert(a.clone(), 1u64);
    assert_eq!(map.get(&b), Some(&1));
    assert_eq!(map.get(&c), None);
}
