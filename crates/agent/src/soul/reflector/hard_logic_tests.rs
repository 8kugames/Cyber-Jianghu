// 天魂 Layer 0（硬性逻辑审查）测试 — 经 validate_pipeline 公共入口驱动
use super::tests::{mock_container, test_world_building_rules, test_world_state};
use super::*;
use crate::component::llm::MockLlmClient;
use crate::soul::reflector::types::ValidationRuntimeConfig;
use cyber_jianghu_protocol::{GatherableItem, WorldState};
use uuid::Uuid;
// ================================================================
// Layer 0：硬性逻辑审查（目标可见性）
// ================================================================

fn approved_mock_validator() -> ReflectorSoul {
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client))
}

/// 构造保证不匹配 entities 中任何 id 的 8 位 hex 前缀
///（翻转 entities[0] 首字符，确定性无碰撞，消除随机 uuid 前缀擞中的 flaky 风险）
fn absent_agent_prefix(world_state: &WorldState) -> String {
    let raw = world_state.entities[0].id.to_string();
    let mut prefix = raw[..8].to_string();
    let flipped = if prefix.as_bytes()[0] == b'0' {
        'f'
    } else {
        '0'
    };
    prefix.replace_range(0..1, &flipped.to_string());
    prefix
}

#[tokio::test]
async fn test_layer0_rejects_item_not_visible() {
    let validator = approved_mock_validator();
    let world_state = test_world_state();

    // 龙肉既不在背包（馒头）、不在附近（木棍）、也不在可采集（空）中
    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": "龙肉"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, layers } => {
            assert!(
                reason.contains("不可见"),
                "应因物品不可见被驳回: {}",
                reason
            );
            let layer0 = layers.first().expect("layer0 应存在");
            assert_eq!(layer0.layer, "layer0");
            assert!(!layer0.passed);
            assert_eq!(layers.len(), 1, "layer0 拒绝后不应继续后续层");
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("不可见物品应被 layer0 拒绝");
        }
    }
}

#[tokio::test]
async fn test_layer0_rejects_fabricated_item_id() {
    // known_item_ids 非空时，臆造物品被存在性检查拦截（区分“臆测”与“不可见”）
    let mut rules = test_world_building_rules();
    rules.known_item_ids = vec!["馒头".to_string(), "木棍".to_string()];
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    let validator = ReflectorSoul::new(rules, mock_container(mock_client));
    let world_state = test_world_state();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": "龙泉剑"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, layers } => {
            assert!(
                reason.contains("不存在于世界物品定义"),
                "臆造物品应被存在性拦截: {}",
                reason
            );
            assert_eq!(layers.first().unwrap().layer, "layer0");
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("臆造物品应被 layer0 拒绝");
        }
    }
}

#[tokio::test]
async fn test_layer0_allows_item_in_inventory() {
    let validator = approved_mock_validator();
    let world_state = test_world_state();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": "馒头"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Approved { layers, .. } => {
            let layer0 = layers.first().expect("layer0 应存在");
            assert_eq!(layer0.layer, "layer0");
            assert!(layer0.passed, "背包物品应通过 layer0");
        }
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("背包物品应通过: {}", reason);
        }
    }
}

#[tokio::test]
async fn test_layer0_allows_item_nearby() {
    let validator = approved_mock_validator();
    let world_state = test_world_state();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "用",
            Some(serde_json::json!({"item_id": "木棍"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Approved { .. } => {}
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("附近物品应通过 layer0: {}", reason);
        }
    }
}

#[tokio::test]
async fn test_layer0_allows_item_gatherable() {
    let validator = approved_mock_validator();
    let mut world_state = test_world_state();
    world_state.location.gatherable_items = vec![GatherableItem {
        // 协议层携带物品 uuid（v5 派生），与新 Server 行为一致
        item_id: cyber_jianghu_protocol::item_uuid("草药").to_string(),
        name: "草药".to_string(),
        item_type: "material".to_string(),
    }];
    let agent_id = world_state.agent_id.unwrap_or_default();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            agent_id,
            world_state.tick_id,
            "取",
            Some(serde_json::json!({
                "source_type": "resource",
                "item_id": "草药",
                "quantity": 1
            })),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Approved { .. } => {}
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("资源点物品应通过 layer0: {}", reason);
        }
    }
}

#[tokio::test]
async fn test_layer0_take_from_agent_skips_visibility() {
    // 取-agent 的目标在对方背包中，本方不可观察，可见性检查应跳过
    let validator = approved_mock_validator();
    let world_state = test_world_state();
    let source_id = world_state.entities[0].id.to_string();
    let agent_id = world_state.agent_id.unwrap_or_default();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            agent_id,
            world_state.tick_id,
            "取",
            Some(serde_json::json!({
                "source_type": "agent",
                "source_id": source_id,
                "item_id": "神秘宝剑",
                "quantity": 1
            })),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Approved { .. } => {}
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("取-agent 应跳过可见性检查: {}", reason);
        }
    }
}

#[tokio::test]
async fn test_layer0_take_from_agent_normalizes_uuid_suffix() {
    // 取-agent + 观察文本的 `名称[短uuid]` 形态：通过且规范化回写裸名
    //（回归审查 F1：取-agent 是照抄后缀概率最高的路径，回写不能被例外分支跳过）
    let validator = approved_mock_validator();
    let world_state = test_world_state();
    let source_id = world_state.entities[0].id.to_string();
    let short = cyber_jianghu_protocol::item_uuid("神秘宝剑").to_string()[..8].to_string();
    let agent_id = world_state.agent_id.unwrap_or_default();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            agent_id,
            world_state.tick_id,
            "取",
            Some(serde_json::json!({
                "source_type": "agent",
                "source_id": source_id,
                "item_id": format!("神秘宝剑[{}]", short),
                "quantity": 1
            })),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Approved { intent, .. } => {
            assert_eq!(
                intent.action_data.as_ref().unwrap().get("item_id"),
                Some(&serde_json::json!(
                    cyber_jianghu_protocol::item_uuid("神秘宝剑").to_string()
                )),
                "取-agent 例外路径同样必须规范化回写完整 uuid"
            );
        }
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("取-agent 带合法 uuid 后缀应通过: {}", reason);
        }
    }
}

#[tokio::test]
async fn test_layer0_acquired_item_ids_count_as_visible() {
    // 链内前序已验证"取"动作获得的物品（不在快照可见集合）应视为可见
    //（回归审查 F3：subsequent_intents 链共享快照，取后即用不能误拦）
    let validator = approved_mock_validator();
    let world_state = test_world_state();
    let agent_id = world_state.agent_id.unwrap_or_default();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            agent_id,
            world_state.tick_id,
            "用",
            Some(serde_json::json!({"item_id": "新拾取的草药"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig {
            // 链内前序取动作回写后为完整 uuid（与新回写语义一致）
            acquired_item_ids: vec![cyber_jianghu_protocol::item_uuid("新拾取的草药").to_string()],
            ..Default::default()
        },
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Approved { .. } => {}
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("链内获得物应视为可见: {}", reason);
        }
    }
}

#[tokio::test]
async fn test_layer0_rejects_target_agent_not_visible() {
    let validator = approved_mock_validator();
    let world_state = test_world_state();
    let absent = absent_agent_prefix(&world_state);

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "攻击",
            Some(serde_json::json!({"target_agent_id": absent})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, layers } => {
            assert!(
                reason.contains("不在附近"),
                "臆测人物目标应被 layer0 拒绝: {}",
                reason
            );
            assert_eq!(layers.first().unwrap().layer, "layer0");
            assert_eq!(layers.len(), 1);
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("臆测人物目标应被拒绝");
        }
    }
}

#[tokio::test]
async fn test_layer0_runs_before_layer1() {
    // 非法 action_type + 臆测 target_agent_id：若 layer0 在前，拒绝原因是目标不可见而非 action 不合法
    let validator = approved_mock_validator();
    let world_state = test_world_state();
    let absent = absent_agent_prefix(&world_state);

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "飞天遁地",
            Some(serde_json::json!({"target_agent_id": absent})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, layers } => {
            assert!(
                reason.contains("不在附近") && !reason.contains("不在合法列表"),
                "layer0 应先于 layer1 拦截: {}",
                reason
            );
            assert_eq!(layers.first().unwrap().layer, "layer0");
        }
        PipelineValidationResult::Approved { .. } => panic!("应被拒绝"),
    }
}

#[tokio::test]
async fn test_layer0_skips_when_world_state_none() {
    let validator = approved_mock_validator();
    let agent_id = Uuid::new_v4();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            agent_id,
            1,
            "吃",
            Some(serde_json::json!({"item_id": "任意物品"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: None,
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Approved { layers, .. } => {
            let layer0 = layers.first().expect("layer0 应存在");
            assert_eq!(layer0.layer, "layer0");
            assert!(layer0.passed, "world_state 缺失时 layer0 应跳过");
        }
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("world_state 缺失时不应拒绝: {}", reason);
        }
    }
}

// ================================================================
// Layer 0：物品 uuid 展示形态（名称[短uuid]）解析与校验
// ================================================================

#[tokio::test]
async fn test_layer0_parses_uuid_suffix_and_normalizes() {
    // LLM 照抄观察/执行文本中的 `馒头[短uuid]` → 校验通过且规范化回写裸名
    let validator = approved_mock_validator();
    let world_state = test_world_state();
    let short = cyber_jianghu_protocol::item_uuid("馒头").to_string()[..8].to_string();
    let raw_ref = format!("馒头[{}]", short);

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": raw_ref})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Approved { intent, .. } => {
            assert_eq!(
                intent.action_data.as_ref().unwrap().get("item_id"),
                Some(&serde_json::json!(
                    cyber_jianghu_protocol::item_uuid("馒头").to_string()
                )),
                "审查通过的 intent 应规范化回写完整 uuid"
            );
        }
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("合法 uuid 形态应通过: {}", reason);
        }
    }
}

#[tokio::test]
async fn test_layer0_rejects_uuid_mismatch() {
    // 短 uuid 与名称派生结果不符 = 臆造/篡造引用，拦截
    // （用另一个物品的派生 uuid 贴给馒头，确定性不匹配）
    let validator = approved_mock_validator();
    let world_state = test_world_state();
    let wrong_short = cyber_jianghu_protocol::item_uuid("刀").to_string()[..8].to_string();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": format!("馒头[{}]", wrong_short)})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, layers } => {
            assert!(
                reason.contains("uuid 与名称不符"),
                "uuid 不匹配应被拦截: {}",
                reason
            );
            assert_eq!(layers.first().unwrap().layer, "layer0");
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("uuid 与名称不符的引用应被拒绝");
        }
    }
}

#[tokio::test]
async fn test_layer0_rejects_fabricated_item_with_uuid_suffix() {
    // 臆造物品 + 正确格式的 uuid 后缀（uuid 一致但物品不存在）→ 存在性检查拦截
    let mut rules = test_world_building_rules();
    rules.known_item_ids = vec!["馒头".to_string()];
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    let validator = ReflectorSoul::new(rules, mock_container(mock_client));
    let world_state = test_world_state();
    let short = cyber_jianghu_protocol::item_uuid("龙泉剑").to_string()[..8].to_string();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": format!("龙泉剑[{}]", short)})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, .. } => {
            assert!(
                reason.contains("不存在于世界物品定义"),
                "臆造物品带 uuid 后缀也应被存在性拦截: {}",
                reason
            );
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("臆造物品应被拒绝");
        }
    }
}

#[tokio::test]
async fn test_layer0_rejects_fabricated_full_uuid_on_take_from_agent() {
    // 臆造完整 uuid + 取-agent 例外路径：存在性校验不能被可见性豁免绕过
    //（形态 1 回归：known 非空时 uuid 必须派生自已知物品）
    let mut rules = test_world_building_rules();
    rules.known_item_ids = vec!["馒头".to_string()];
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    let validator = ReflectorSoul::new(rules, mock_container(mock_client));
    let world_state = test_world_state();
    let source_id = world_state.entities[0].id.to_string();
    let fabricated = cyber_jianghu_protocol::item_uuid("臆造物品").to_string();
    let agent_id = world_state.agent_id.unwrap_or_default();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            agent_id,
            world_state.tick_id,
            "取",
            Some(serde_json::json!({
                "source_type": "agent",
                "source_id": source_id,
                "item_id": fabricated,
                "quantity": 1
            })),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, layers } => {
            assert!(
                reason.contains("不存在于世界物品定义"),
                "臆造完整 uuid 应被存在性拦截: {}",
                reason
            );
            assert_eq!(layers.first().unwrap().layer, "layer0");
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("取-agent 例外不能绕过臆造 uuid 拦截");
        }
    }
}
