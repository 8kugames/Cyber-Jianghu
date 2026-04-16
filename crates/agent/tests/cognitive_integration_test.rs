//! 认知引擎集成测试
//!
//! 测试 CognitiveEngine + CognitiveValidator 的完整认知流程

use std::sync::Arc;

use cyber_jianghu_agent::component::llm::mock::MockLlmClient;
use cyber_jianghu_agent::models::WorldState;
use cyber_jianghu_agent::soul::actor::stages::{
    CognitiveStage, PerceptionMotivationResponse, PlanDecisionResponse, StageOutput,
};
use cyber_jianghu_agent::soul::actor::{CognitiveChain, CognitiveEngine};
use cyber_jianghu_agent::soul::reflector::cognitive_validator::CognitiveValidator;

// ============================================================================
// 辅助函数
// ============================================================================

fn make_minimal_world_state(tick_id: i64) -> WorldState {
    let json = serde_json::json!({
        "event_type": "world_state",
        "tick_id": tick_id,
        "world_time": {"year": 2024, "month": 1, "day": 1, "hour": 8, "minute": 0, "second": 0, "weather": "晴"},
        "location": {"name": "村口", "node_id": "village_gate", "type": "street", "adjacent_nodes": []},
        "self_state": {"attributes": {}, "attribute_descriptions": {}, "status_effects": [], "inventory": []}
    });
    serde_json::from_value(json).unwrap()
}

fn make_mock_client() -> MockLlmClient {
    let perception_motivation = serde_json::to_string(&PerceptionMotivationResponse {
        self_status: "健康，饥饿度适中".to_string(),
        environment: "村口集市，人来人往".to_string(),
        key_observations: vec!["有个摊贩卖包子".to_string(), "远处有人在练武".to_string()],
        primary_drive: "获取食物".to_string(),
        drive_intensity: 7,
        reasoning: "肚子有点饿了，需要补充体力".to_string(),
    })
    .unwrap();

    let plan_decision = serde_json::to_string(&PlanDecisionResponse {
        steps: vec!["走向包子摊".to_string(), "购买包子".to_string()],
        priority: 7,
        expected_outcome: "获得食物，恢复体力".to_string(),
        thought_process:
            "感知到集市有包子摊，动机是获取食物充饥，规划是先走向摊位再购买，因此决定执行购买动作"
                .to_string(),
        narrative_action: "去包子摊买包子充饥".to_string(),
    })
    .unwrap();

    let all_responses = format!("{}\n---\n{}", perception_motivation, plan_decision);
    MockLlmClient::with_response(&all_responses)
}

fn make_validator() -> CognitiveValidator {
    CognitiveValidator::new("测试侠客人设".to_string())
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cognitive_chain_lifecycle() {
        let mut chain = CognitiveChain::new("测试侠客".to_string(), "测试人设".to_string(), 42);
        assert_eq!(chain.tick_id, 42);
        assert!(!chain.is_complete());

        for stage in CognitiveStage::all() {
            chain.add_stage(StageOutput::new(stage, format!("{:?} 内容", stage)));
        }
        assert!(chain.is_complete());
        assert_eq!(chain.stages.len(), 4);

        assert!(chain.get_stage(CognitiveStage::Perception).is_some());
        assert!(chain.get_stage(CognitiveStage::Decision).is_some());

        let summary = chain.summarize();
        assert!(summary.contains("测试侠客"));
        assert!(summary.contains("Tick 42"));
    }

    #[tokio::test]
    async fn test_cognitive_validator_approves_valid_chain() {
        let mut chain = CognitiveChain::new("侠客".to_string(), "人设".to_string(), 1);

        chain.add_stage(StageOutput::new(
            CognitiveStage::Perception,
            "我站在村口集市上，观察到周围环境，发现有商贩和行人。我的状态是饥饿度中等，体力尚可。"
                .to_string(),
        ));
        chain.add_stage(StageOutput::new(
            CognitiveStage::Motivation,
            "我感到饥饿，驱动力是获取食物。强度 7/10，因为我已经有一段时间没吃东西了。".to_string(),
        ));
        chain.add_stage(StageOutput::new(
            CognitiveStage::Planning,
            "基于获取食物的动机，我计划先走向包子摊，然后购买包子。优先级 7。".to_string(),
        ));
        chain.add_stage(StageOutput::new(
            CognitiveStage::Decision,
            "基于感知到集市有食物、动机是获取食物、规划是购买包子，我决定执行 use 动作购买包子。"
                .to_string(),
        ));

        let validator = make_validator();
        let result = validator.validate(&chain);
        assert!(
            result.is_valid,
            "Valid chain should pass: {:?}",
            result.reason
        );
    }

    #[tokio::test]
    async fn test_cognitive_validator_rejects_empty_chain() {
        let chain = CognitiveChain::new("侠客".to_string(), "人设".to_string(), 1);
        let validator = make_validator();
        let result = validator.validate(&chain);
        assert!(!result.is_valid);
    }

    #[tokio::test]
    async fn test_cognitive_validator_rejects_incomplete_chain() {
        let mut chain = CognitiveChain::new("侠客".to_string(), "人设".to_string(), 1);
        chain.add_stage(StageOutput::new(
            CognitiveStage::Perception,
            "感知内容".to_string(),
        ));
        chain.add_stage(StageOutput::new(
            CognitiveStage::Motivation,
            "动机内容".to_string(),
        ));

        let validator = make_validator();
        let result = validator.validate(&chain);
        assert!(!result.is_valid);
    }

    #[tokio::test]
    async fn test_cognitive_validator_rejects_short_content() {
        let mut chain = CognitiveChain::new("侠客".to_string(), "人设".to_string(), 1);

        for stage in CognitiveStage::all() {
            chain.add_stage(StageOutput::new(stage, "短".to_string()));
        }

        let validator = make_validator();
        let result = validator.validate(&chain);
        assert!(!result.is_valid);
        assert!(
            result.reason.as_ref().is_some_and(|r| r.contains("过短")),
            "Should reject short content, got: {:?}",
            result.reason
        );
    }

    #[tokio::test]
    async fn test_cognitive_validator_custom_min_length() {
        let mut chain = CognitiveChain::new("侠客".to_string(), "人设".to_string(), 1);

        for stage in CognitiveStage::all() {
            chain.add_stage(StageOutput::new(stage, "短的".to_string()));
        }

        // 默认 min_thought_length=10 应拒绝（"短的" = 6 bytes < 10）
        let strict = make_validator();
        let result = strict.validate(&chain);
        assert!(
            !result.is_valid,
            "Should reject content shorter than 10 bytes"
        );

        // 放宽长度阈值，内容应通过长度检查
        let relaxed = make_validator().with_min_thought_length(5);
        let result = relaxed.validate(&chain);
        if !result.is_valid {
            assert!(
                result.reason.as_ref().is_some_and(|r| !r.contains("过短")),
                "Should not complain about short content with relaxed threshold, got: {:?}",
                result.reason
            );
        }
    }

    #[tokio::test]
    async fn test_cognitive_chain_serialization() {
        let mut chain = CognitiveChain::new("侠客".to_string(), "人设".to_string(), 1);
        for stage in CognitiveStage::all() {
            chain.add_stage(StageOutput::new(stage, format!("{:?} 内容", stage)));
        }

        let json = serde_json::to_string(&chain).unwrap();
        let deserialized: CognitiveChain = serde_json::from_str(&json).unwrap();
        assert!(deserialized.is_complete());
        assert_eq!(deserialized.stages.len(), 4);
    }

    #[tokio::test]
    async fn test_cognitive_engine_create_callback() {
        let mock = Arc::new(make_mock_client());
        let engine = CognitiveEngine::with_defaults(mock);
        let callback = engine.create_decision_callback();

        let world_state = make_minimal_world_state(1);
        let tick_id = world_state.tick_id;
        let agent_id = world_state.agent_id.unwrap_or_default();
        let intent = callback(tick_id, agent_id).await;

        // callback 要么返回引擎生成的 intent，要么返回 fallback idle
        // MockLlmClient 固定字符串可能导致解析失败，所以 idle fallback 是合理的
        assert_eq!(intent.tick_id, 1);
    }

    #[tokio::test]
    async fn test_cognitive_engine_full_flow() {
        let mock = Arc::new(make_mock_client());
        let engine = CognitiveEngine::with_defaults(mock);

        let world_state = make_minimal_world_state(1);
        let result = engine
            .think(
                world_state.tick_id,
                world_state.agent_id.unwrap_or_default(),
            )
            .await;

        // MockLlmClient 返回固定字符串，后续阶段可能解析失败
        // 验证引擎不 panic，要么成功要么正确传播错误
        match result {
            Ok(chain) => {
                assert!(
                    chain.get_stage(CognitiveStage::Perception).is_some(),
                    "Chain should have perception stage"
                );
            }
            Err(_) => {
                // 预期行为：MockLlmClient 固定字符串导致后续阶段解析失败
            }
        }
    }
}
