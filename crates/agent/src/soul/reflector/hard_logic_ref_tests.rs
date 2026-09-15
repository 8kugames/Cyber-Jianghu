// 天魂 Layer 0 扩展测试：形态 4（短 uuid 前缀解析）、空 item_id 自愈、可照抄预览形态
use super::tests::{mock_container, test_world_building_rules, test_world_state};
use super::*;
use crate::component::llm::MockLlmClient;
use crate::soul::reflector::types::ValidationRuntimeConfig;

fn approved_mock_validator() -> ReflectorSoul {
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client))
}

// ================================================================
// Layer 0：形态 4（纯短 uuid 前缀）、空 item_id 自愈、可照抄预览
// ================================================================

/// 构造与「馒头」派生 uuid 前 8 位确定性不匹配的 hex 前缀（翻转首字符 0<->f）
fn absent_item_prefix() -> String {
    let raw = cyber_jianghu_protocol::item_uuid("馒头").to_string();
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
async fn test_layer0_resolves_short_uuid_prefix() {
    // 形态 4：纯短 uuid 前缀在世界定义内唯一匹配 → 自动派生完整 uuid 回写
    let mut rules = test_world_building_rules();
    rules.known_item_ids = vec!["馒头".to_string(), "木棍".to_string()];
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    let validator = ReflectorSoul::new(rules, mock_container(mock_client));
    let world_state = test_world_state();

    let short = cyber_jianghu_protocol::item_uuid("馒头").to_string()[..8].to_string();
    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": short})),
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
                "短 uuid 前缀应规范化回写完整 uuid"
            );
        }
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("唯一匹配的短 uuid 前缀应通过: {}", reason);
        }
    }
}

#[tokio::test]
async fn test_layer0_rejects_unknown_short_uuid_prefix() {
    // 形态 4 零匹配：确定性不匹配前缀 → 明确拒绝并给出合法物品清单
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
            Some(serde_json::json!({"item_id": absent_item_prefix()})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, .. } => {
            assert!(
                reason.contains("不匹配任何世界物品定义"),
                "零匹配短 uuid 应被拒绝: {}",
                reason
            );
            assert!(
                reason.contains("馒头"),
                "拒绝消息应列出合法物品: {}",
                reason
            );
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("零匹配短 uuid 前缀应被拒绝");
        }
    }
}

#[tokio::test]
async fn test_layer0_empty_item_id_rejected_with_guidance() {
    // 空 item_id：专属消息（而非误导性的「物品「」不存在」），附背包可照抄预览
    let validator = approved_mock_validator();
    let world_state = test_world_state();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": ""})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, .. } => {
            assert!(
                reason.contains("item_id 为空"),
                "空 item_id 应有专属指引: {}",
                reason
            );
            assert!(
                reason.contains("馒头["),
                "空 item_id 消息应含可照抄背包预览: {}",
                reason
            );
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("空 item_id 应被拒绝（auto_fill 默认关闭）");
        }
    }
}

#[tokio::test]
async fn test_layer0_auto_fill_unique_inventory_item() {
    // 空 item_id + auto_fill 开启 + 背包唯一候选 → 零 token 回填放行
    let validator = approved_mock_validator();
    let world_state = test_world_state();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": "  "})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig {
            auto_fill_unique_item: true,
            ..Default::default()
        },
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Approved { intent, .. } => {
            assert_eq!(
                intent.action_data.as_ref().unwrap().get("item_id"),
                Some(&serde_json::json!(
                    cyber_jianghu_protocol::item_uuid("馒头").to_string()
                )),
                "空 item_id 应回填唯一背包物品的完整 uuid"
            );
        }
        PipelineValidationResult::Rejected { reason, .. } => {
            panic!("唯一候选 auto-fill 应放行: {}", reason);
        }
    }
}

#[tokio::test]
async fn test_layer0_rejection_previews_use_display_ref() {
    // 可见性拒绝消息：headline 与背包预览均为「名称[短uuid]」可照抄形态
    let mut rules = test_world_building_rules();
    rules.known_item_ids = vec!["馒头".to_string(), "木棍".to_string()];
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    let validator = ReflectorSoul::new(rules, mock_container(mock_client));
    let world_state = test_world_state();

    // 木棍存在于世界定义但不在背包 → Inventory 可见性拒绝
    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": "木棍"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, .. } => {
            let stick_short =
                cyber_jianghu_protocol::item_uuid("木棍").to_string()[..8].to_string();
            let mantou_short =
                cyber_jianghu_protocol::item_uuid("馒头").to_string()[..8].to_string();
            assert!(
                reason.contains(&format!("木棍[{}]", stick_short)),
                "headline 应用名称[短uuid] 形态: {}",
                reason
            );
            assert!(
                reason.contains(&format!("馒头[{}]", mantou_short)),
                "背包预览应用名称[短uuid] 形态: {}",
                reason
            );
            assert!(
                reason.contains("禁止自造英文 ID"),
                "拒绝消息应附带照抄指引: {}",
                reason
            );
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("不在背包的物品应被 Inventory 可见性拦截");
        }
    }
}

// ================================================================
// 建议8 补测：形态 4 多匹配 / known 空宽口径 / auto-fill 多候选 / hex 边界
// ================================================================

#[tokio::test]
async fn test_layer0_rejects_ambiguous_short_uuid_prefix() {
    // 形态 4 多匹配：生日碰撞搜索两个 v5 派生 uuid 前 4 位 hex 相同的物品名
    // （65536 前缀空间，5000 样本碰撞概率趋近 1，毫秒级完成）
    use std::collections::HashMap;
    let mut seen: HashMap<String, String> = HashMap::new();
    let (mut name_a, mut name_b) = (String::new(), String::new());
    'outer: for i in 0..5000u32 {
        let name = format!("测物{i}");
        let prefix = cyber_jianghu_protocol::item_uuid(&name).to_string()[..4].to_string();
        if let Some(prev) = seen.get(&prefix) {
            name_a = prev.clone();
            name_b = name;
            break 'outer;
        }
        seen.insert(prefix, name);
    }
    assert!(!name_a.is_empty(), "碰撞搜索应找到前 4 位相同的两个名字");

    let mut rules = test_world_building_rules();
    rules.known_item_ids = vec![name_a.clone(), name_b.clone()];
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    let validator = ReflectorSoul::new(rules, mock_container(mock_client));
    let world_state = test_world_state();

    let short4 = cyber_jianghu_protocol::item_uuid(&name_a).to_string()[..4].to_string();
    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": short4})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, .. } => {
            assert!(
                reason.contains("匹配到多个物品"),
                "多匹配短 uuid 应被歧义拒绝: {}",
                reason
            );
            assert!(
                reason.contains(&name_a) && reason.contains(&name_b),
                "歧义消息应列出候选的可照抄形态: {}",
                reason
            );
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("多匹配短 uuid 前缀应被拒绝");
        }
    }
}

#[tokio::test]
async fn test_layer0_known_empty_hex_prefix_wide_semantics() {
    // known 为空（规则未下发）：hex 前缀不触发形态 4，走宽口径按裸名派生——
    // 跳过存在性拦截，最终因不可见被拒（而非「不存在」），与旧宽口径行为一致
    let validator = approved_mock_validator();
    let world_state = test_world_state();

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": "a65df604"})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig::default(),
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, .. } => {
            assert!(
                reason.contains("不可见"),
                "known 空时 hex 串应按裸名宽口径派生并进入可见性检查: {}",
                reason
            );
            assert!(
                !reason.contains("不存在于世界物品定义"),
                "宽口径不应触发存在性拦截: {}",
                reason
            );
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("不可见物品应被 Inventory 可见性拦截");
        }
    }
}

#[tokio::test]
async fn test_layer0_auto_fill_skipped_when_multiple_candidates() {
    // auto-fill 开启但背包多候选 → 不回填，走空 item_id 专属拒绝
    let validator = approved_mock_validator();
    let mut world_state = test_world_state();
    world_state
        .self_state
        .inventory
        .push(cyber_jianghu_protocol::InventoryItem {
            item_id: cyber_jianghu_protocol::item_uuid("木棍").to_string(),
            name: "木棍".to_string(),
            quantity: 1,
            is_equipped: false,
            item_type: "weapon".to_string(),
        });

    let request = ValidationRequest {
        intent: crate::models::Intent::new(
            world_state.agent_id.unwrap_or_default(),
            world_state.tick_id,
            "吃",
            Some(serde_json::json!({"item_id": " "})),
        ),
        persona: PersonaInfo::default(),
        world_context: "测试地点".to_string(),
        world_state: Some(world_state),
        runtime: ValidationRuntimeConfig {
            auto_fill_unique_item: true,
            ..Default::default()
        },
    };

    match validator.validate_pipeline(request).await.unwrap() {
        PipelineValidationResult::Rejected { reason, .. } => {
            assert!(
                reason.contains("item_id 为空"),
                "多候选不回填，应走专属拒绝消息: {}",
                reason
            );
        }
        PipelineValidationResult::Approved { .. } => {
            panic!("多候选时空 item_id 不应被自动回填");
        }
    }
}

#[tokio::test]
async fn test_layer0_hex_prefix_length_boundaries() {
    // is_hex_prefix 边界：3 位（过短）/ 17 位（过长）/ 含非 hex 字符
    // 均不触发形态 4，落入未知名称的存在性拒绝
    let mut rules = test_world_building_rules();
    rules.known_item_ids = vec!["馒头".to_string()];
    let mock_client =
        MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
    let validator = ReflectorSoul::new(rules, mock_container(mock_client));
    let world_state = test_world_state();

    for bad in ["abc", "a65df604a65df604a", "abcy"] {
        let request = ValidationRequest {
            intent: crate::models::Intent::new(
                world_state.agent_id.unwrap_or_default(),
                world_state.tick_id,
                "吃",
                Some(serde_json::json!({"item_id": bad})),
            ),
            persona: PersonaInfo::default(),
            world_context: "测试地点".to_string(),
            world_state: Some(world_state.clone()),
            runtime: ValidationRuntimeConfig::default(),
        };

        match validator.validate_pipeline(request).await.unwrap() {
            PipelineValidationResult::Rejected { reason, .. } => {
                assert!(
                    reason.contains("不存在于世界物品定义"),
                    "'{}' 不满足前缀条件，应走存在性拒绝: {}",
                    bad,
                    reason
                );
            }
            PipelineValidationResult::Approved { .. } => {
                panic!("'{}' 不应通过存在性校验", bad);
            }
        }
    }
}
