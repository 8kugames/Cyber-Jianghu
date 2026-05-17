// ============================================================================
// ReflectorSoul（天魂/守护之魂）— 出口验证器
// ============================================================================
//
// 天魂的出向职责：三层审查 Intent，确保合法后才提交 server。
// ============================================================================

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::component::llm::LlmClientExt;
use crate::infra::api::thinking_log;
use crate::runtime::claw::LlmClientContainer;
use crate::soul::actor::prompt_template::PromptTemplateConfig;
use cyber_jianghu_protocol::{GradedValidationConfig, WorldBuildingRules};

use super::prompt::ObserverPrompt;
use super::rule_engine::{RuleEngine, RuleValidationContext, types::extract_ids_from_world_state};
use super::types::{
    LayerResult, LlmValidationResponse, PersonaInfo, PipelineValidationResult, RejectionType,
    ValidationRequest, ValidationResult, ValidationRuntimeConfig,
};

// ============================================================================
// 验证器 Trait（类型擦除）
// ============================================================================

/// 验证器 Trait（用于 Agent 存储）
///
/// 提供类型擦除，允许 Agent 存储任意 LlmClient 实现的验证器
#[async_trait]
pub trait Validator: Send + Sync {
    /// 验证意图
    async fn validate(&self, request: ValidationRequest) -> Result<PipelineValidationResult>;

    /// 验证人设
    async fn validate_persona(&self, persona: &PersonaInfo) -> Result<ValidationResult>;

    /// 更新世界观规则
    async fn update_rules(&self, rules: WorldBuildingRules);

    /// 更新 reject 反馈模板
    fn update_prompt_config(&self, _config: Arc<PromptTemplateConfig>) {}
}

/// 为 ReflectorSoul 实现 Validator trait
#[async_trait]
impl Validator for ReflectorSoul {
    async fn validate(&self, request: ValidationRequest) -> Result<PipelineValidationResult> {
        self.validate_pipeline(request).await
    }

    async fn validate_persona(&self, persona: &PersonaInfo) -> Result<ValidationResult> {
        self.validate_persona(persona).await
    }

    async fn update_rules(&self, rules: WorldBuildingRules) {
        self.update_rules(rules).await
    }

    fn update_prompt_config(&self, config: Arc<PromptTemplateConfig>) {
        self.update_prompt_config(config);
    }
}

/// ReflectorSoul — 意图审查引擎
///
/// 同步串联在认知链路中，ActorSoul 生成的 Intent 必须经过审查才能提交。
/// 单次结构化 LLM 调用，无 retry 循环。
pub struct ReflectorSoul {
    /// 世界观规则
    rules: Arc<RwLock<WorldBuildingRules>>,
    /// LLM 客户端容器（支持热重载）
    llm_container: LlmClientContainer,
    /// 观察者 prompt 模板
    observer_prompt: ObserverPrompt,
    /// Layer 2 规则引擎
    rule_engine: RuleEngine,
}

impl ReflectorSoul {
    /// 创建新的 ReflectorSoul
    pub fn new(rules: WorldBuildingRules, llm_container: LlmClientContainer) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
            llm_container,
            observer_prompt: ObserverPrompt::new(),
            rule_engine: RuleEngine::with_default_config(),
        }
    }

    /// 暴露 RuleEngine 的 reject 模板配置句柄，供生命周期回调热更新
    pub fn prompt_config_handle(
        &self,
    ) -> Arc<
        std::sync::RwLock<Option<Arc<crate::soul::actor::prompt_template::PromptTemplateConfig>>>,
    > {
        self.rule_engine.prompt_config_handle()
    }

    /// 从 Server 下发更新 reject 反馈模板
    pub fn update_prompt_config(
        &self,
        config: Arc<crate::soul::actor::prompt_template::PromptTemplateConfig>,
    ) {
        self.rule_engine.update_prompt_config(config);
    }

    /// 判断 Intent 是否应跳过 LLM 审核（分级审核策略）
    pub fn should_skip_llm_validation(
        intent: &crate::models::Intent,
        config: Option<&GradedValidationConfig>,
    ) -> bool {
        let Some(config) = config else {
            return false;
        };

        let action_str = intent.action_type.as_str().to_string();
        if config.skip_types.contains(&action_str) {
            return true;
        }
        if config.always_types.contains(&action_str) {
            return false;
        }
        if config.adaptive_types.contains(&action_str) {
            return !Self::adaptive_needs_llm(intent, config);
        }

        true
    }

    fn adaptive_needs_llm(intent: &crate::models::Intent, config: &GradedValidationConfig) -> bool {
        let action_data = match &intent.action_data {
            Some(d) => d,
            None => return false,
        };

        if let Some(field_name) = config
            .adaptive_field_mapping
            .get(intent.action_type.as_str())
        {
            match field_name.as_str() {
                "target_location" => action_data
                    .get(field_name)
                    .and_then(|v| v.as_str())
                    .map(|loc| {
                        config
                            .restricted_area_keywords
                            .iter()
                            .any(|k| loc.contains(k.as_str()))
                    })
                    .unwrap_or(false),
                "item_id" => action_data
                    .get(field_name)
                    .and_then(|v| v.as_str())
                    .map(|id| {
                        config
                            .high_value_item_keywords
                            .iter()
                            .any(|k| id.contains(k.as_str()))
                    })
                    .unwrap_or(false),
                _ => true,
            }
        } else {
            true
        }
    }

    /// Layer 1：确定性 action_type 校验
    fn validate_action_type(
        &self,
        intent: &crate::models::Intent,
    ) -> std::result::Result<(), String> {
        if intent.action_type.as_str() == "休息" {
            return Ok(());
        }

        if intent.action_type.as_str() == "narrative" {
            tracing::error!(
                "narrative sentinel 泄漏到 ReflectorSoul.validate_action_type，强制拒绝"
            );
            return Err("意图格式异常：narrative 未被翻译".to_string());
        }

        let actions = crate::infra::api::cognitive_context::load_available_actions_from_file();
        if actions.is_empty() {
            return Ok(());
        }

        // 查找匹配的 action 定义
        let action_input = intent.action_type.as_str().to_lowercase();
        let matched = actions.iter().find(|a| {
            a.action == intent.action_type.as_str()
                || a.name.to_lowercase() == action_input
                || a.aliases
                    .iter()
                    .any(|alias| alias.to_lowercase() == action_input)
        });

        if let Some(action) = matched {
            // 校验 required_fields
            for field in &action.required_fields {
                let has_field = intent
                    .action_data
                    .as_ref()
                    .and_then(|d| d.get(field))
                    .map(|v| !v.is_null())
                    .unwrap_or(false);
                if !has_field {
                    return Err(format!(
                        "动作 '{}' 缺少必需字段: {}",
                        action.name, field
                    ));
                }
            }
            return Ok(());
        }

        let valid_names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();

        let suggestion = actions
            .iter()
            .find(|action| {
                let name_lower = action.name.to_lowercase();
                name_lower.contains(&action_input) || action_input.contains(&name_lower)
            })
            .map(|action| action.name.as_str())
            .unwrap_or("休息");

        Err(format!(
            "action '{}' 不在合法列表中，合法值: [{}]，最接近: '{}'",
            intent.action_type,
            valid_names.join(", "),
            suggestion,
        ))
    }

    /// Layer 2：RuleEngine 规则校验
    async fn validate_with_rule_engine(
        &self,
        request: &ValidationRequest,
        consecutive_follow_count: usize,
        max_consecutive_follow: usize,
    ) -> std::result::Result<(), String> {
        if request.intent.action_type.as_str() == "follow"
            && consecutive_follow_count >= max_consecutive_follow
        {
            return Err(format!(
                "已连续跟随 {} 次，请尝试其他行为（如 说话、采集、休息）",
                max_consecutive_follow
            ));
        }

        let Some(world_state) = request.world_state.as_ref() else {
            return Ok(());
        };

        let (available_item_ids, reachable_node_ids) = extract_ids_from_world_state(world_state);
        let context = RuleValidationContext {
            intent: request.intent.clone(),
            persona_info: request.persona.clone(),
            world_context: request.world_context.clone(),
            tick_id: world_state.tick_id,
            history_intents: vec![],
            attributes: std::collections::HashMap::new(),
            available_item_ids,
            reachable_node_ids,
        };

        match self.rule_engine.validate_context(&context).await {
            Ok(ValidationResult::Approved { .. }) => Ok(()),
            Ok(ValidationResult::Rejected { reason, .. }) => Err(reason),
            Err(e) => {
                tracing::warn!("RuleEngine error, bypassing: {}", e);
                Ok(())
            }
        }
    }

    /// Layer 1/2/3 统一出口
    pub async fn validate_pipeline(
        &self,
        request: ValidationRequest,
    ) -> Result<PipelineValidationResult> {
        let ValidationRuntimeConfig {
            graded_config,
            consecutive_follow_count,
            max_consecutive_follow,
        } = request.runtime.clone();
        let mut layers = Vec::with_capacity(3);

        match self.validate_action_type(&request.intent) {
            Ok(()) => layers.push(LayerResult {
                layer: "layer1",
                passed: true,
                detail: None,
            }),
            Err(reason) => {
                layers.push(LayerResult {
                    layer: "layer1",
                    passed: false,
                    detail: Some(reason.clone()),
                });
                return Ok(PipelineValidationResult::Rejected { reason, layers });
            }
        }

        match self
            .validate_with_rule_engine(&request, consecutive_follow_count, max_consecutive_follow)
            .await
        {
            Ok(()) => layers.push(LayerResult {
                layer: "layer2",
                passed: true,
                detail: None,
            }),
            Err(reason) => {
                layers.push(LayerResult {
                    layer: "layer2",
                    passed: false,
                    detail: Some(reason.clone()),
                });
                return Ok(PipelineValidationResult::Rejected { reason, layers });
            }
        }

        if request.intent.chaos_marker.is_some()
            || Self::should_skip_llm_validation(&request.intent, graded_config.as_ref())
        {
            layers.push(LayerResult {
                layer: "layer3",
                passed: true,
                detail: Some("llm validation skipped".to_string()),
            });
            return Ok(PipelineValidationResult::Approved {
                intent: request.intent,
                layers,
                narrative: None,
            });
        }

        let llm_result = match self.validate_llm(request.clone()).await {
            Ok(result) => result,
            Err(e) => {
                layers.push(LayerResult {
                    layer: "layer3",
                    passed: true,
                    detail: Some(format!("LLM error, bypassed: {}", e)),
                });
                return Ok(PipelineValidationResult::Approved {
                    intent: request.intent,
                    layers,
                    narrative: None,
                });
            }
        };

        match llm_result {
            ValidationResult::Approved { narrative, .. } => {
                layers.push(LayerResult {
                    layer: "layer3",
                    passed: true,
                    detail: None,
                });
                Ok(PipelineValidationResult::Approved {
                    intent: request.intent,
                    layers,
                    narrative: if narrative.is_empty() {
                        None
                    } else {
                        Some(narrative)
                    },
                })
            }
            ValidationResult::Rejected {
                reason,
                rejection_type,
            } => {
                if matches!(rejection_type, RejectionType::OutOfCharacter)
                    && let Some(world_state) = request.world_state.as_ref()
                {
                    const SURVIVAL_OVERRIDE_THRESHOLD: i32 = 40;
                    let hunger = world_state.self_state.hunger();
                    let thirst = world_state.self_state.thirst();
                    if hunger < SURVIVAL_OVERRIDE_THRESHOLD || thirst < SURVIVAL_OVERRIDE_THRESHOLD
                    {
                        layers.push(LayerResult {
                            layer: "layer3",
                            passed: true,
                            detail: Some(format!(
                                "survival_override: hunger={}, thirst={} < {}",
                                hunger, thirst, SURVIVAL_OVERRIDE_THRESHOLD
                            )),
                        });
                        return Ok(PipelineValidationResult::Approved {
                            intent: request.intent,
                            layers,
                            narrative: None,
                        });
                    }
                }

                layers.push(LayerResult {
                    layer: "layer3",
                    passed: false,
                    detail: Some(reason.clone()),
                });
                Ok(PipelineValidationResult::Rejected { reason, layers })
            }
        }
    }

    /// 仅执行 LLM 审查
    async fn validate_llm(&self, request: ValidationRequest) -> Result<ValidationResult> {
        let rules = self.rules.read().await.clone();

        // 构建验证 prompt
        let prompt = self.observer_prompt.build_validation_prompt(
            &request.intent,
            &request.persona,
            &rules,
            &request.world_context,
        );

        debug!("Validation prompt:\n{}", prompt);

        // 调用 LLM（从 container 读取当前客户端，支持热重载）
        let llm_client = self.llm_container.read().await.clone();
        let response: LlmValidationResponse = llm_client
            .complete_json_with_system(self.observer_prompt.system_prompt(), &prompt)
            .await?;

        thinking_log::log_llm(
            &format!("Agent({})", request.intent.agent_id),
            request.intent.tick_id,
            "ReflectorSoul",
            &prompt,
            &format!("{:?}", response),
        );

        let result = response.into_validation_result();

        match &result {
            ValidationResult::Approved { reason, narrative } => {
                info!(
                    "Intent approved, reason: {:?}, narrative: {}",
                    reason, narrative
                );
            }
            ValidationResult::Rejected {
                reason,
                rejection_type,
            } => {
                warn!("Intent rejected: {} [{:?}]", reason, rejection_type);
            }
        }

        Ok(result)
    }

    /// 更新世界观规则（服务端广播时调用）
    ///
    /// 仅当新规则版本号大于当前版本时才更新
    pub async fn update_rules(&self, rules: WorldBuildingRules) {
        let mut current_rules = self.rules.write().await;

        // 版本检查：跳过旧版本或相同版本
        if rules.version <= current_rules.version {
            info!(
                "WorldBuildingRules update skipped: current={}, received={}",
                current_rules.version, rules.version
            );
            return;
        }

        *current_rules = rules;
        info!(
            "WorldBuildingRules updated to version {}",
            current_rules.version
        );
    }

    /// 验证人设（注册阶段，客户端本地验证）
    pub async fn validate_persona(&self, persona: &PersonaInfo) -> Result<ValidationResult> {
        let rules = self.rules.read().await.clone();

        let prompt = format!(
            r#"## 世界观规则

### 时代设定
- 时代：{}
- 技术水平：{}

### 禁止的概念
{}

## 人设验证请求

请验证以下人设是否符合世界观设定：

- 角色：{}
- 性别：{}
- 年龄：{}
- 性格：{}
- 价值观：{}

注意：人设中的性格和价值观不应包含现代概念、魔法元素或穿越者知识。角色名字是游戏设定，不属于穿越概念。角色可以拥有与历史人物相同的名字。

请按以下 JSON 格式输出：
{{
  "result": "approved" | "rejected",
  "reason": "通过/驳回的原因",
  "rejection_type": "era_violation" | "other"
}}"#,
            rules.era.name,
            rules.era.tech_level,
            rules.forbidden_concepts.join("、"),
            persona.name.as_deref().unwrap_or("未命名"),
            persona.gender,
            persona.age,
            persona.personality.join("、"),
            persona.values.join("、"),
        );

        let llm_client = self.llm_container.read().await.clone();
        let response: LlmValidationResponse = llm_client
            .complete_json_with_system(self.observer_prompt.system_prompt(), &prompt)
            .await?;

        Ok(response.into_validation_result())
    }
}

// ============================================================================
// LLM 响应转换
// ============================================================================

impl LlmValidationResponse {
    /// 转换为内部 ValidationResult
    pub fn into_validation_result(self) -> ValidationResult {
        match self.result.as_str() {
            "approved" => ValidationResult::Approved {
                reason: if self.reason.is_empty() {
                    None
                } else {
                    Some(self.reason)
                },
                narrative: self.narrative,
            },
            "rejected" => ValidationResult::Rejected {
                reason: self.reason,
                rejection_type: super::types::RejectionType::parse(&self.rejection_type),
            },
            _ => {
                // 空 result 或无法识别 → 宽容策略：降级为通过
                // 避免 LLM 截断导致验证拒绝，触发无意义的重试循环
                tracing::warn!(
                    "Unrecognized validation result '{}', auto-approving (lenient policy). raw='{}', narrative='{}'",
                    self.result,
                    self.reason,
                    self.narrative
                );
                ValidationResult::Approved {
                    reason: None,
                    narrative: self.narrative,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::llm::MockLlmClient;
    use cyber_jianghu_protocol::{
        AdjacentNode, AgentSelfState, Entity, GradedValidationConfig, InventoryItem, Location,
        SceneItem, WorldState, WorldTime,
    };
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use uuid::Uuid;

    fn mock_container(client: MockLlmClient) -> LlmClientContainer {
        Arc::new(RwLock::new(Arc::new(client)))
    }

    fn test_world_building_rules() -> WorldBuildingRules {
        use cyber_jianghu_protocol::EraSettings;
        WorldBuildingRules {
            version: "0.0.1-test".to_string(),
            era: EraSettings {
                name: "武侠架空世界".to_string(),
                tech_level: "冷兵器时代".to_string(),
                social_structure: "封建帝制".to_string(),
            },
            allowed_concepts: vec!["内力".to_string(), "轻功".to_string()],
            forbidden_concepts: vec!["魔法".to_string()],
            narrative_rules: "测试叙事规则".to_string(),
            last_updated: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn test_world_state() -> WorldState {
        let mut attributes = HashMap::new();
        attributes.insert("hunger".to_string(), 80);
        attributes.insert("thirst".to_string(), 80);

        WorldState {
            event_type: "world_state".to_string(),
            tick_id: 1,
            agent_id: Some(Uuid::new_v4()),
            world_time: WorldTime {
                year: 1,
                month: 1,
                day: 1,
                hour: 8,
                minute: 0,
                second: 0,
                weather: "晴".to_string(),
            },
            location: Location {
                node_id: "loc_a".to_string(),
                name: "地点A".to_string(),
                node_type: "inn".to_string(),
                adjacent_nodes: vec![AdjacentNode {
                    node_id: "loc_b".to_string(),
                    name: "地点B".to_string(),
                    travel_cost: 1,
                    aliases: vec![],
                }],
                gatherable_items: vec![],
            },
            self_state: AgentSelfState {
                attributes,
                derived_attributes: HashMap::new(),
                attribute_descriptions: HashMap::new(),
                status_effects: vec![],
                inventory: vec![InventoryItem {
                    item_id: "馒头".to_string(),
                    name: "馒头".to_string(),
                    item_type: "food".to_string(),
                    quantity: 1,
                    is_equipped: false,
                    aliases: vec![],
                }],
                skills: vec![],
                age_years: None,
                max_age: None,
                recipe_details: vec![],
            },
            entities: vec![Entity {
                id: Uuid::new_v4(),
                name: "路人甲".to_string(),
                distance: 0,
                state: "alive".to_string(),
                hostile: false,
                recent_actions: vec![],
            }],
            nearby_items: vec![SceneItem {
                item_id: "木棍".to_string(),
                name: "木棍".to_string(),
                item_type: "weapon".to_string(),
                quantity: 1,
                aliases: vec![],
            }],
            events_log: vec![],
            private_dialogue_log: vec![],
            last_execution_summary: None,
            lessons_learned: vec![],
        }
    }

    #[tokio::test]
    async fn test_validate_approved() {
        let mock_client = MockLlmClient::with_response(
            r#"{
            "result": "approved",
            "reason": "行为符合武侠世界观",
            "narrative": "李四决定在客栈休息"
        }"#,
        );

        let validator =
            ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client));

        let request = ValidationRequest {
            intent: crate::models::Intent::new(uuid::Uuid::new_v4(), 1, "休息", None),
            persona: PersonaInfo::default(),
            world_context: "龙门客栈".to_string(),
            world_state: None,
            runtime: ValidationRuntimeConfig::default(),
        };

        let result = validator.validate(request).await.unwrap();

        match result {
            PipelineValidationResult::Approved { narrative, .. } => {
                assert_eq!(narrative, Some("李四决定在客栈休息".to_string()));
            }
            _ => panic!("Expected Approved"),
        }
    }

    #[tokio::test]
    async fn test_validate_rejected() {
        let mock_client = MockLlmClient::with_response(
            r#"{
            "result": "rejected",
            "reason": "使用了魔法，违反力量体系",
            "rejection_type": "power_system_violation"
        }"#,
        );

        let validator =
            ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client));

        let request = ValidationRequest {
            intent: crate::models::Intent::new(uuid::Uuid::new_v4(), 1, "休息", None),
            persona: PersonaInfo::default(),
            world_context: "龙门客栈".to_string(),
            world_state: None,
            runtime: ValidationRuntimeConfig::default(),
        };

        let result = validator.validate(request).await.unwrap();

        match result {
            PipelineValidationResult::Rejected { reason, .. } => {
                assert_eq!(reason, "使用了魔法，违反力量体系");
            }
            _ => panic!("Expected Rejected"),
        }
    }

    #[tokio::test]
    async fn test_update_rules() {
        let mock_client = MockLlmClient::with_response(
            r#"{"result": "approved", "reason": "", "narrative": ""}"#,
        );

        let validator =
            ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client));

        // Test that update_rules doesn't panic
        let new_rules = test_world_building_rules();
        validator.update_rules(new_rules).await;
    }

    #[tokio::test]
    async fn test_validate_pipeline_rejects_follow_loop_before_llm() {
        let mock_client =
            MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
        let reflector =
            ReflectorSoul::new(test_world_building_rules(), mock_container(mock_client));
        let world_state = test_world_state();
        let request = ValidationRequest {
            intent: crate::models::Intent::new(
                world_state.agent_id.unwrap_or_default(),
                world_state.tick_id,
                "follow",
                Some(serde_json::json!({"target_agent_id": Uuid::new_v4()})),
            ),
            persona: PersonaInfo::default(),
            world_context: "测试地点".to_string(),
            world_state: Some(world_state),
            runtime: ValidationRuntimeConfig {
                graded_config: Some(GradedValidationConfig::default()),
                consecutive_follow_count: 3,
                max_consecutive_follow: 3,
            },
        };

        let result = reflector.validate_pipeline(request).await.unwrap();

        match result {
            PipelineValidationResult::Rejected { reason, layers } => {
                assert!(reason.contains("已连续跟随 3 次"));
                assert_eq!(layers.len(), 2);
                assert_eq!(layers[0].layer, "layer1");
                assert!(layers[0].passed);
                assert_eq!(layers[1].layer, "layer2");
                assert!(!layers[1].passed);
            }
            PipelineValidationResult::Approved { .. } => {
                panic!("follow loop should be rejected before llm");
            }
        }
    }

    #[tokio::test]
    async fn test_validator_trait_runs_full_pipeline() {
        let mock_client =
            MockLlmClient::with_response(r#"{"result":"approved","reason":"","narrative":"通过"}"#);
        let validator: Arc<dyn Validator> = Arc::new(ReflectorSoul::new(
            test_world_building_rules(),
            mock_container(mock_client),
        ));
        let world_state = test_world_state();
        let request = ValidationRequest {
            intent: crate::models::Intent::new(
                world_state.agent_id.unwrap_or_default(),
                world_state.tick_id,
                "follow",
                Some(serde_json::json!({"target_agent_id": Uuid::new_v4()})),
            ),
            persona: PersonaInfo::default(),
            world_context: "测试地点".to_string(),
            world_state: Some(world_state),
            runtime: ValidationRuntimeConfig {
                graded_config: Some(GradedValidationConfig::default()),
                consecutive_follow_count: 3,
                max_consecutive_follow: 3,
            },
        };

        match validator.validate(request).await.unwrap() {
            PipelineValidationResult::Rejected { reason, layers } => {
                assert!(reason.contains("已连续跟随 3 次"));
                assert_eq!(layers.len(), 2);
                assert_eq!(layers[1].layer, "layer2");
            }
            PipelineValidationResult::Approved { .. } => {
                panic!("trait validator should run full pipeline");
            }
        }
    }
}
