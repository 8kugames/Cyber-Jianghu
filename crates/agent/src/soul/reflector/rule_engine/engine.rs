//! 规则引擎核心
//!
//! 提供规则验证的统一入口点，协调注册表和评估器。

use super::evaluator::{ConditionEvaluator, DefaultEvaluator};
use super::registry::{RuleRegistry, RuleSet};
use super::types::{Rule, RuleValidationContext, extract_ids_from_world_state};
use crate::soul::actor::prompt_template::PromptTemplateConfig;
use crate::soul::reflector::{
    LayerResult, PersonaInfo, PipelineValidationResult, RejectionType, ValidationRequest,
    ValidationResult, Validator,
};
use async_trait::async_trait;
use cyber_jianghu_protocol::WorldBuildingRules;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

// ============================================================================
// RuleEngine 错误消息常量
// ============================================================================
// 集中定义，供 narrativize_rejection() 引用，避免 string.contains 紧耦合

/// eat item_id 无效
pub const ERR_EAT_INVALID_ITEM: &str = "吃东西失败：物品ID无效";
/// drink item_id 无效
pub const ERR_DRINK_INVALID_ITEM: &str = "喝水失败：物品ID无效";
/// move target_location 无效
pub const ERR_MOVE_INVALID_TARGET: &str = "移动失败：目标地点ID无效";

/// 规则引擎
///
/// 协调规则注册表和条件评估器，提供统一的验证入口
/// reject 反馈模板共享状态（RuleEngine + WS 回调共享同一 Arc）
type SharedPromptConfig = std::sync::RwLock<Option<Arc<PromptTemplateConfig>>>;

pub struct RuleEngine {
    /// 规则注册表
    registry: Arc<RuleRegistry>,
    /// 条件评估器
    evaluator: Box<dyn ConditionEvaluator>,
    /// reject 反馈模板配置（Arc<RwLock> 支持跨组件热更新共享）
    prompt_config: Arc<SharedPromptConfig>,
}

#[async_trait]
impl Validator for RuleEngine {
    async fn validate(
        &self,
        request: ValidationRequest,
    ) -> anyhow::Result<PipelineValidationResult> {
        // 构建验证上下文
        let tick_id = request.intent.tick_id;
        let (available_item_ids, reachable_node_ids) = request
            .world_state
            .as_ref()
            .map(extract_ids_from_world_state)
            .unwrap_or_default();

        let context = RuleValidationContext {
            intent: request.intent,
            persona_info: request.persona,
            world_context: request.world_context,
            tick_id,
            history_intents: vec![],
            attributes: HashMap::new(),
            available_item_ids,
            reachable_node_ids,
        };

        // 调用内部验证逻辑
        match self.validate_context(&context).await? {
            ValidationResult::Approved { .. } => Ok(PipelineValidationResult::Approved {
                intent: context.intent,
                layers: vec![
                    LayerResult {
                        layer: "layer1",
                        passed: true,
                        detail: Some("rule_engine_only".to_string()),
                    },
                    LayerResult {
                        layer: "layer2",
                        passed: true,
                        detail: Some("rule_engine_only".to_string()),
                    },
                ],
                narrative: None,
            }),
            ValidationResult::Rejected { reason, .. } => Ok(PipelineValidationResult::Rejected {
                reason: reason.clone(),
                layers: vec![
                    LayerResult {
                        layer: "layer1",
                        passed: true,
                        detail: Some("rule_engine_only".to_string()),
                    },
                    LayerResult {
                        layer: "layer2",
                        passed: false,
                        detail: Some(reason),
                    },
                ],
            }),
        }
    }

    async fn validate_persona(&self, _persona: &PersonaInfo) -> anyhow::Result<ValidationResult> {
        // 规则引擎暂时不验证人设，直接通过
        Ok(ValidationResult::Approved {
            reason: None,
            narrative: String::new(),
        })
    }

    async fn update_rules(&self, _rules: WorldBuildingRules) {
        // ReflectorSoul::update_rules() 直接调用 self.rule_engine.reload_rules_from_json()
        // 此 trait 方法无需额外操作
    }
}

impl RuleEngine {
    /// 创建新的规则引擎（使用默认评估器）
    pub fn new() -> Self {
        Self {
            registry: Arc::new(RuleRegistry::new()),
            evaluator: Box::new(DefaultEvaluator),
            prompt_config: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    /// 创建带有默认配置的规则引擎（空规则集，等待 Server 下发）
    ///
    /// 规则从 Server 通过 WorldBuildingRules.rules_json 下发，
    /// ReflectorSoul::update_rules() 调用 reload_rules_from_json() 加载。
    pub fn with_default_config() -> Self {
        Self {
            registry: Arc::new(RuleRegistry::new()),
            evaluator: Box::new(DefaultEvaluator),
            prompt_config: Arc::new(std::sync::RwLock::new(Self::load_prompt_config())),
        }
    }

    /// 从 JSON 配置创建规则引擎
    pub fn from_config(rules_json: &serde_json::Value) -> Self {
        let rules: Vec<Rule> = serde_json::from_value(rules_json.clone()).unwrap_or_default();
        let mut rule_set = RuleSet::new();
        for rule in rules {
            rule_set.add_rule(rule);
        }
        Self {
            registry: Arc::new(RuleRegistry::from_rule_set(rule_set)),
            evaluator: Box::new(DefaultEvaluator),
            prompt_config: Arc::new(std::sync::RwLock::new(Self::load_prompt_config())),
        }
    }

    /// 运行时从 JSON 重载规则（通过 RuleRegistry 内部 RwLock 原子替换）
    pub async fn reload_rules_from_json(&self, rules_json: serde_json::Value) {
        let rules: Vec<Rule> = serde_json::from_value(rules_json).unwrap_or_default();
        let count = rules.len();
        self.registry.replace_all(rules).await;
        tracing::info!("RuleEngine 已从配置重载 {} 条规则", count);
    }

    /// 使用自定义评估器创建规则引擎
    pub fn with_evaluator<E>(mut self, evaluator: E) -> Self
    where
        E: ConditionEvaluator + 'static,
    {
        self.evaluator = Box::new(evaluator);
        self
    }

    /// 加载 reject 反馈模板配置
    /// 第一优先级：CYBER_JIANGHU_DATA_DIR（Server 写入路径，与 Server 写盘目标对称）
    fn load_prompt_config() -> Option<Arc<PromptTemplateConfig>> {
        let search_paths: Vec<Option<std::path::PathBuf>> = vec![
            std::env::var("CYBER_JIANGHU_DATA_DIR")
                .ok()
                .map(|d| std::path::PathBuf::from(d).join("prompt_templates.json")),
            std::env::var("CYBER_JIANGHU_CONFIG_DIR")
                .ok()
                .map(|d| std::path::PathBuf::from(d).join("prompt_templates.json")),
            dirs::home_dir().map(|h| {
                h.join(".cyber-jianghu")
                    .join("config")
                    .join("prompt_templates.json")
            }),
            Some(std::path::PathBuf::from("config/prompt_templates.json")),
        ];

        for path_opt in &search_paths {
            if let Some(path) = path_opt
                && path.exists()
            {
                match super::super::super::actor::prompt_template::load_prompt_template_from_file(
                    path,
                ) {
                    Ok(config) => {
                        info!("RuleEngine 已加载 reject 反馈模板: {:?}", path);
                        return Some(Arc::new(config));
                    }
                    Err(e) => {
                        warn!(
                            "RuleEngine prompt 模板文件解析失败 ({}): {}，等待 Server 下发",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }
        None
    }

    /// 获取规则注册表的引用
    pub fn registry(&self) -> Arc<RuleRegistry> {
        Arc::clone(&self.registry)
    }

    /// 获取 prompt_config 共享句柄（供 WS 回调等外部组件直接写入）
    pub fn prompt_config_handle(&self) -> Arc<SharedPromptConfig> {
        Arc::clone(&self.prompt_config)
    }

    /// 从 Server 下发更新 prompt 配置
    pub fn update_prompt_config(&self, config: Arc<PromptTemplateConfig>) {
        let mut guard = self.prompt_config.write().expect("rwlock poisoned");
        *guard = Some(config);
    }

    /// 增强 reject 消息：附加上下文数据帮助 LLM 自纠正
    ///
    /// 有模板配置时使用数据驱动模板，否则 fallback 到基础增强。
    fn enhance_rejection(
        &self,
        rule_id: &str,
        base_reason: &str,
        context: &RuleValidationContext,
    ) -> String {
        let action_type = match rule_id {
            "valid_item_id_eat" => "进食",
            "valid_item_id_drink" => "饮水",
            "valid_target_node_move" => "移动",
            _ => return base_reason.to_string(),
        };

        // 尝试使用模板配置
        let guard = self.prompt_config.read().expect("rwlock poisoned");
        if let Some(config) = guard.as_ref()
            && let Some(tmpl) = config.get_template("reject_feedback")
        {
            let max_items = config.truncation("reject_feedback", "max_items", 5);
            let mut vars = HashMap::new();

            match action_type {
                "进食" | "饮水" => {
                    let items: Vec<&str> = context
                        .available_item_ids
                        .iter()
                        .take(max_items)
                        .map(|s| s.as_str())
                        .collect();
                    vars.insert(
                        "available_items".to_string(),
                        if items.is_empty() {
                            "（背包为空，请先 pickup 或 gather）".to_string()
                        } else {
                            items.join(", ")
                        },
                    );
                }
                "移动" => {
                    let nodes: Vec<&str> = context
                        .reachable_node_ids
                        .iter()
                        .take(max_items)
                        .map(|s| s.as_str())
                        .collect();
                    vars.insert(
                        "reachable_nodes".to_string(),
                        if nodes.is_empty() {
                            "（当前无可达地点）".to_string()
                        } else {
                            nodes.join(", ")
                        },
                    );
                }
                _ => {}
            }

            if let Some(rendered) = tmpl.render_section(action_type, &vars) {
                return rendered.trim().to_string();
            }
        }

        // Fallback：基础增强（无模板时）
        base_reason.to_string()
    }

    /// 验证意图（内部方法）
    ///
    /// 对所有启用的规则进行验证，如果任何规则失败则返回 Rejected
    pub async fn validate_context(
        &self,
        context: &RuleValidationContext,
    ) -> anyhow::Result<ValidationResult> {
        // 获取所有启用的规则
        let rules = self.registry.all_enabled().await;

        tracing::debug!("开始验证，共 {} 条规则", rules.len());

        // 如果没有规则，直接通过
        if rules.is_empty() {
            tracing::debug!("没有启用的规则，直接通过验证");
            return Ok(ValidationResult::Approved {
                reason: None,
                narrative: String::new(),
            });
        }

        // 自引用检查：不能对自己使用定向动作
        if let Some(ref action_data) = context.intent.action_data
            && let Some(target_id) = action_data.get("target_agent_id").and_then(|v| v.as_str())
            && target_id == context.intent.agent_id.to_string()
        {
            return Ok(ValidationResult::Rejected {
                reason: "不能对自己使用该动作，请选择附近的他人作为目标".to_string(),
                rejection_type: RejectionType::Other,
            });
        }

        // 逐条评估规则
        for rule in &rules {
            let rule_result = self.evaluate_rule(rule, context).await?;

            if !rule_result.passed {
                let base_reason = rule_result
                    .error_message
                    .unwrap_or_else(|| format!("规则 {} 验证失败", rule.name));

                let enhanced_reason = self.enhance_rejection(&rule.id, &base_reason, context);

                tracing::warn!("规则验证失败: {} - {}", rule.id, enhanced_reason);

                // 规则失败，返回 Rejected
                return Ok(ValidationResult::Rejected {
                    reason: enhanced_reason,
                    rejection_type: RejectionType::Other,
                });
            }

            tracing::debug!("规则验证通过: {}", rule.id);
        }

        // 所有规则通过
        Ok(ValidationResult::Approved {
            reason: Some(format!("所有 {} 条规则验证通过", rules.len())),
            narrative: String::new(),
        })
    }

    /// 评估单个规则
    pub async fn evaluate_rule(
        &self,
        rule: &Rule,
        context: &RuleValidationContext,
    ) -> anyhow::Result<super::types::RuleValidationResult> {
        // 防御性检查：跳过未启用的规则
        if !rule.enabled {
            tracing::debug!("规则已禁用，跳过评估: {}", rule.id);
            return Ok(super::types::RuleValidationResult::passed(rule.id.clone()));
        }

        // 使用评估器评估规则条件
        let passed = self.evaluator.evaluate(&rule.condition, context).await;

        if passed {
            Ok(super::types::RuleValidationResult::passed(rule.id.clone()))
        } else {
            Ok(super::types::RuleValidationResult::failed(
                rule.id.clone(),
                rule.error_message.clone(),
            ))
        }
    }
}

impl Default for RuleEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Intent;
    use crate::soul::reflector::types::PersonaInfo;
    use cyber_jianghu_protocol::ActionType;
    use std::collections::HashMap;
    use uuid::Uuid;

    /// 创建测试用的验证上下文
    fn create_test_context() -> RuleValidationContext {
        let agent_id = Uuid::new_v4();
        let intent = Intent::new(
            agent_id,
            10,
            ActionType::SPEAK,
            Some(serde_json::json!({"content": "hello world"})),
        );

        let mut attributes = HashMap::new();
        attributes.insert("health".to_string(), serde_json::json!(100));
        attributes.insert("level".to_string(), serde_json::json!(5));

        RuleValidationContext {
            intent,
            persona_info: PersonaInfo::default(),
            world_context: String::new(),
            tick_id: 10,
            history_intents: vec![],
            attributes,
            available_item_ids: vec![],
            reachable_node_ids: vec![],
        }
    }

    #[tokio::test]
    async fn test_validate_no_rules() {
        let engine = RuleEngine::new();
        let context = create_test_context();

        // 没有规则时应该直接通过
        let result = engine.validate_context(&context).await.unwrap();

        match result {
            ValidationResult::Approved { reason, narrative } => {
                assert!(reason.is_none());
                assert!(narrative.is_empty());
            }
            ValidationResult::Rejected { .. } => panic!("应该通过验证，但被拒绝了"),
        }
    }

    #[tokio::test]
    async fn test_validate_failing_rule() {
        let engine = RuleEngine::new();
        let registry = engine.registry();

        // 注册一个会失败的规则（动作类型不是 "移动"）
        let rule = Rule::new(
            "test_rule_1".to_string(),
            "动作必须是 移动".to_string(),
            super::super::types::RuleType::ActionCooldown,
            super::super::types::RuleCondition::Equals(
                "intent.action_type".to_string(),
                serde_json::json!("移动"),
            ),
            "动作类型必须是 移动".to_string(),
        );

        registry.register(rule).await;

        let context = create_test_context();
        let result = engine.validate_context(&context).await.unwrap();

        match result {
            ValidationResult::Approved { .. } => {
                panic!("应该被拒绝，但通过了验证");
            }
            ValidationResult::Rejected { reason, .. } => {
                assert!(reason.contains("移动") || reason.contains("动作类型"));
            }
        }
    }

    #[tokio::test]
    async fn test_validate_passing_rule() {
        let engine = RuleEngine::new();
        let registry = engine.registry();

        // 注册一个会通过的规则（动作类型是 "说话"）
        let rule = Rule::new(
            "test_rule_2".to_string(),
            "动作必须是 说话".to_string(),
            super::super::types::RuleType::ActionCooldown,
            super::super::types::RuleCondition::Equals(
                "intent.action_type".to_string(),
                serde_json::json!("说话"),
            ),
            "动作类型必须是 说话".to_string(),
        );

        registry.register(rule).await;

        let context = create_test_context();
        let result = engine.validate_context(&context).await.unwrap();

        match result {
            ValidationResult::Approved { reason, .. } => {
                assert!(reason.is_some());
                assert!(reason.as_ref().unwrap().contains("通过"));
            }
            ValidationResult::Rejected { reason, .. } => {
                panic!("应该通过验证，但被拒绝了: {}", reason);
            }
        }
    }

    #[tokio::test]
    async fn test_from_config_semantic_equivalence() {
        // 模拟 rules.json 内容
        let rules_json: serde_json::Value = serde_json::json!([
            {
                "id": "valid_item_id_eat",
                "name": "eat 的 item_id 必须在背包中",
                "rule_type": "ResourceConstraint",
                "condition": {
                    "Or": [
                        {"NotEquals": ["intent.action_type", "进食"]},
                        {"In": ["intent.action_data.item_id", "available_item_ids"]}
                    ]
                },
                "error_message": "吃东西失败：物品ID无效，请使用背包中物品的精确ID",
                "enabled": true
            },
            {
                "id": "valid_item_id_drink",
                "name": "drink 的 item_id 必须在背包中",
                "rule_type": "ResourceConstraint",
                "condition": {
                    "Or": [
                        {"NotEquals": ["intent.action_type", "饮水"]},
                        {"In": ["intent.action_data.item_id", "available_item_ids"]}
                    ]
                },
                "error_message": "喝水失败：物品ID无效，请使用背包中物品的精确ID",
                "enabled": true
            },
            {
                "id": "valid_target_node_move",
                "name": "move 的 target_location 必须可达",
                "rule_type": "StateRestriction",
                "condition": {
                    "Or": [
                        {"NotEquals": ["intent.action_type", "移动"]},
                        {"In": ["intent.action_data.target_location", "reachable_node_ids"]}
                    ]
                },
                "error_message": "移动失败：目标地点ID无效，请使用可达地点的精确ID",
                "enabled": true
            }
        ]);

        let engine = RuleEngine::from_config(&rules_json);

        // eat + 有效 item_id → 通过
        let mut ctx = create_test_context();
        ctx.intent = Intent::new(
            Uuid::new_v4(),
            1,
            "进食",
            Some(serde_json::json!({"item_id": "馒头"})),
        );
        ctx.available_item_ids = vec!["馒头".to_string()];
        let result = engine.validate_context(&ctx).await.unwrap();
        assert!(
            matches!(result, ValidationResult::Approved { .. }),
            "eat 有效 item_id 应通过"
        );

        // eat + 无效 item_id → 拒绝
        ctx.available_item_ids = vec!["水".to_string()];
        let result = engine.validate_context(&ctx).await.unwrap();
        assert!(
            matches!(result, ValidationResult::Rejected { .. }),
            "eat 无效 item_id 应拒绝"
        );

        // 非进食动作 → 通过（蕴含式放行）
        ctx.intent = Intent::new(Uuid::new_v4(), 1, "说话", None);
        let result = engine.validate_context(&ctx).await.unwrap();
        assert!(
            matches!(result, ValidationResult::Approved { .. }),
            "非进食动作应通过"
        );

        // move + 有效 target → 通过
        ctx.intent = Intent::new(
            Uuid::new_v4(),
            1,
            "移动",
            Some(serde_json::json!({"target_location": "龙门厨房"})),
        );
        ctx.reachable_node_ids = vec!["龙门厨房".to_string()];
        let result = engine.validate_context(&ctx).await.unwrap();
        assert!(
            matches!(result, ValidationResult::Approved { .. }),
            "move 有效 target 应通过"
        );

        // move + 无效 target → 拒绝
        ctx.reachable_node_ids = vec!["龙门后院".to_string()];
        let result = engine.validate_context(&ctx).await.unwrap();
        assert!(
            matches!(result, ValidationResult::Rejected { .. }),
            "move 无效 target 应拒绝"
        );
    }

    #[tokio::test]
    async fn test_reload_rules_from_json() {
        let engine = RuleEngine::new();

        // 初始空规则 → 通过
        let ctx = create_test_context();
        let result = engine.validate_context(&ctx).await.unwrap();
        assert!(matches!(result, ValidationResult::Approved { .. }));

        // 加载规则：禁止进食（NotEquals 蕴含式）
        let rules_json = serde_json::json!([
            {
                "id": "block_eat",
                "name": "禁止进食",
                "rule_type": "ActionCooldown",
                "condition": {"NotEquals": ["intent.action_type", "进食"]},
                "error_message": "禁止进食",
                "enabled": true
            }
        ]);
        let parsed: Vec<Rule> = serde_json::from_value(rules_json.clone()).unwrap();
        assert_eq!(parsed.len(), 1, "应解析出 1 条规则");

        engine.reload_rules_from_json(rules_json).await;

        let loaded = engine.registry().all_enabled().await;
        assert_eq!(loaded.len(), 1, "应加载 1 条规则");

        // 说话（非进食）→ NotEquals 满足 → 通过
        let result = engine.validate_context(&ctx).await.unwrap();
        assert!(
            matches!(result, ValidationResult::Approved { .. }),
            "说话应通过（非进食）"
        );

        // 进食 → NotEquals 不满足 → 拒绝
        let mut ctx_eat = create_test_context();
        ctx_eat.intent = Intent::new(Uuid::new_v4(), 1, "进食", None);
        let result = engine.validate_context(&ctx_eat).await.unwrap();
        assert!(
            matches!(result, ValidationResult::Rejected { .. }),
            "进食应被拒绝"
        );
    }
}
