//! engine 模块单测（自 engine.rs 外移，内容未改）

use super::*;
use crate::governance::types::{
    ReviewPolicy, SoulConfig, SoulsClassifierConfig, SoulsReviewConfig,
};

/// 测试配置：仅伏羲
fn test_souls_config() -> SoulsConfig {
    let mut souls = HashMap::new();
    souls.insert(
        "fuxi".to_string(),
        SoulConfig {
            display_name: "伏羲".to_string(),
            governance_role: "evolution".to_string(),
            review_policy: ReviewPolicy::default(),
            system_prompt_template: "fuxi_review_prompt".to_string(),
        },
    );

    SoulsConfig {
        souls,
        topic_to_soul: [("evolution".to_string(), "fuxi".to_string())]
            .into_iter()
            .collect(),
        topic_priority: [("evolution".to_string(), 0)].into_iter().collect(),
        classifier: SoulsClassifierConfig {
            confidence_threshold: 0.6,
            default_fallback_topic: "evolution".to_string(),
        },
        review: SoulsReviewConfig {
            timeout_secs: 1800,
            dissent_log_threshold: 3,
            approve_threshold: 2,
            poll_interval_secs: 60,
            group_stale_secs: 1800,
        },
    }
}

fn test_engine() -> SoulReviewEngine {
    SoulReviewEngine {
        config: test_souls_config(),
        sources: HashMap::new(),
        llm_client: Arc::new(GovernanceLlmClient {
            enabled: false,
            config: None,
        }),
        capability_manifest: Arc::new(tokio::sync::RwLock::new(CapabilityManifest::default())),
    }
}

#[test]
fn test_route_primary_soul() {
    let engine = test_engine();
    assert_eq!(
        engine.route_primary_soul(&GovernanceTopic::Evolution),
        Some("fuxi".to_string())
    );
    // 仅伏羲注册，其他 topic 返回 None
    assert_eq!(engine.route_primary_soul(&GovernanceTopic::Resource), None);
}

#[test]
fn test_route_for_topics() {
    let engine = test_engine();
    // 仅 evolution → fuxi
    let result = engine.route_for_topics(&[GovernanceTopic::Evolution]);
    assert_eq!(result, Some("fuxi".to_string()));
}
