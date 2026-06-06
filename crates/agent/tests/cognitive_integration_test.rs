//! 认知引擎集成测试
//!
//! 测试 CognitiveEngine + CognitiveValidator 的完整认知流程

use std::path::PathBuf;
use std::sync::Arc;

use cyber_jianghu_agent::component::llm::mock::MockLlmClient;
use cyber_jianghu_agent::component::persona::rules_loader::load_event_trait_rules;
use cyber_jianghu_agent::component::persona::{DynamicPersona, ThreadSafePersona};
use cyber_jianghu_agent::models::{WorldEvent, WorldEventType};
use cyber_jianghu_agent::soul::actor::prompt_template::PromptTemplateConfig;
use cyber_jianghu_agent::soul::actor::stages::{
    CognitiveStage, StageOutput,
};
use cyber_jianghu_agent::soul::actor::{CognitiveChain, CognitiveEngine};
use cyber_jianghu_agent::soul::reflector::cognitive_validator::CognitiveValidator;

// ============================================================================
// 辅助函数
// ============================================================================

fn integration_yaml_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates/server/config/persona_event_rules.yaml")
}

fn make_validator() -> CognitiveValidator {
    CognitiveValidator::new("测试侠客人设".to_string())
}

/// 创建包含 actor_direct 最小模板的 PromptTemplateConfig（测试用）
fn make_minimal_prompt_config() -> PromptTemplateConfig {
    let json = serde_json::json!({
        "version": "test-1.0",
        "templates": {
            "actor_direct": {
                "required_sections": ["persona", "world_state"],
                "sections": {
                    "persona": "{feedback_section}你是{agent_name}，一名江湖中人。\n### 人设\n{persona}\n",
                    "world_state": "### 当前世界状态\n{world_state_section}\n",
                    "memory": "{memory_section}",
                    "summary": "{summary_context}",
                    "actions": "### 可用行动\n{action_descriptions}\n\n{action_field_hints}\n\n{skill_instructions}\n",
                    "output": "{tool_calling_guidance}```json\n{{\"self_status\": \"状态\", \"environment\": \"环境\", \"key_observations\": [\"观察\"], \"primary_drive\": \"驱动力\", \"drive_intensity\": 5, \"thought_process\": \"思考\", \"actions\": [{{\"action_type\": \"...\", \"action_data\": {{}}}}]}}\n```",
                    "outcome": "{outcome_section}"
                }
            }
        }
    });
    PromptTemplateConfig::from_json_value(json).unwrap()
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

    // ========================================================================
    // CU-5: DynamicPersona lifecycle 接线测试
    // ========================================================================
    // 验证 CU-2 / CU-3a / CU-3b 的核心数据流:
    //   process_events → EventTraitMapper.apply_to_persona → persona trait 变更
    //   update_tick_state → apply_all_decay + invalidate_persona_cache
    //
    // 这些都是 Agent 内部的薄壳调用,通过 ThreadSafePersona.write 闭包触发。
    // 直接在集成测试里复现闭包调用模式,验证 wiring 正确无误。

    fn make_attacked_event(tick_id: i64) -> WorldEvent {
        let mut metadata = serde_json::Map::new();
        metadata.insert("targets".to_string(), serde_json::json!(["攻击者"]));
        WorldEvent {
            event_type: WorldEventType::ActionResult,
            tick_id,
            description: "被攻击者攻击".to_string(),
            metadata: serde_json::Value::Object(metadata),
        }
    }

    #[test]
    fn test_event_trait_mapper_through_thread_safe_persona() {
        // 模拟 Agent.process_events 末尾的闭包模式
        let agent_id = uuid::Uuid::new_v4();
        let persona = ThreadSafePersona::new(DynamicPersona::new(agent_id, "测试侠客", "基础描述"));
        let mapper = Arc::new(load_event_trait_rules(&integration_yaml_path()).expect("YAML 加载"));

        // 初始: get_trait("愤怒") 因 default_traits() 不含"愤怒" 而返回 None,
        // 走 .unwrap_or(50) 默认 50
        let initial_anger = persona.read(|p| p.get_trait("愤怒").unwrap_or(50));
        assert_eq!(initial_anger, 50, "愤怒默认值应为 50");

        let event = make_attacked_event(1);
        let mapper_clone = mapper.clone();
        persona.write(|p| {
            mapper_clone.apply_to_persona(&event, p, 1);
        });

        let after_anger = persona.read(|p| p.get_trait("愤怒").unwrap_or(0));
        assert!(
            after_anger > initial_anger,
            "被攻击后愤怒应增加，初始={}, 攻击后={}",
            initial_anger,
            after_anger
        );
        assert!(
            after_anger >= 65,
            "愤怒权重 1.2 + base_delta 15 → 至少 65, 实际={}",
            after_anger
        );
    }

    #[test]
    fn test_apply_all_decay_after_event() {
        // 模拟 update_tick_state 末尾的闭包模式
        let agent_id = uuid::Uuid::new_v4();
        let persona = ThreadSafePersona::new(DynamicPersona::new(agent_id, "测试侠客", "基础描述"));
        let mapper = Arc::new(load_event_trait_rules(&integration_yaml_path()).expect("YAML 加载"));

        let event = make_attacked_event(1);
        let m = mapper.clone();
        persona.write(|p| m.apply_to_persona(&event, p, 1));

        let anger_after_attack = persona.read(|p| p.get_trait("愤怒").unwrap_or(0));
        assert!(anger_after_attack > 50, "攻击后愤怒应增加");

        persona.write(|p| p.apply_all_decay());
        let anger_after_decay = persona.read(|p| p.get_trait("愤怒").unwrap_or(0));

        assert!(
            anger_after_decay < anger_after_attack,
            "tick 2 衰减后愤怒应下降，攻击后={}, 衰减后={}",
            anger_after_attack,
            anger_after_decay
        );
        assert!(
            anger_after_decay >= 50,
            "衰减不应低于基线 50，攻击后={}, 衰减后={}",
            anger_after_attack,
            anger_after_decay
        );
    }

    #[test]
    fn test_cognitive_engine_invalidate_persona_cache() {
        // 验证 CU-3b: CognitiveEngine.invalidate_persona_cache 公开方法可用
        let agent_id = uuid::Uuid::new_v4();
        let persona = ThreadSafePersona::new(DynamicPersona::new(agent_id, "测试侠客", "基础描述"));
        let mapper = Arc::new(load_event_trait_rules(&integration_yaml_path()).expect("YAML 加载"));

        let event = make_attacked_event(1);
        let m = mapper.clone();
        persona.write(|p| m.apply_to_persona(&event, p, 1));

        let mock = Arc::new(MockLlmClient::with_response("{}"));
        let config = cyber_jianghu_agent::soul::actor::CognitiveEngineConfig {
            agent_name: "测试侠客".to_string(),
            temperature: 0.7,
            max_tokens_per_stage: 1024,
        };
        let engine = CognitiveEngine::new(mock, config, &persona);
        engine.update_prompt_template_from_config(make_minimal_prompt_config());

        let post_summary =
            cyber_jianghu_agent::soul::actor::prompt_cache::PromptCache::build_structured_summary(
                &persona.read(|p| p.clone()),
            );
        engine.invalidate_persona_cache(&persona);
        let _post_invalidate =
            cyber_jianghu_agent::soul::actor::prompt_cache::PromptCache::build_structured_summary(
                &persona.read(|p| p.clone()),
            );

        assert!(
            post_summary.contains("愤怒"),
            "summary 应包含攻击产生的'愤怒'特质"
        );
    }
}
