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
use cyber_jianghu_protocol::{GradedValidationConfig, WorldBuildingRules, WorldState};

use super::prompt::ReflectorPrompt;
use super::rule_engine::{RuleEngine, RuleValidationContext, types::extract_ids_from_world_state};
use super::types::{
    LayerResult, LlmValidationResponse, PersonaInfo, PipelineValidationResult, RejectionType,
    ValidationRequest, ValidationResult,
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
    /// ReflectorSoul prompt 模板
    reflector_prompt: ReflectorPrompt,
    /// Layer 2 规则引擎
    rule_engine: RuleEngine,
}

impl ReflectorSoul {
    /// 创建新的 ReflectorSoul
    pub fn new(rules: WorldBuildingRules, llm_container: LlmClientContainer) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
            llm_container,
            reflector_prompt: ReflectorPrompt::new(),
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
        world_state: Option<&WorldState>,
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
            return !Self::adaptive_needs_llm(intent, config, world_state);
        }

        true
    }

    fn adaptive_needs_llm(
        intent: &crate::models::Intent,
        config: &GradedValidationConfig,
        world_state: Option<&WorldState>,
    ) -> bool {
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
                "item_id" => {
                    // 全面 uuid 化后 item_id 为完整 uuid，关键词改对物品名匹配：
                    // 按 uuid 反查 WorldState（背包/附近/采集点）中的名称；
                    // 反查不到（链内新获得物/未知引用）→ 保守升级 LLM 审查
                    let item_ref = action_data
                        .get(field_name)
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let item_name = world_state.and_then(|ws| {
                        ws.self_state
                            .inventory
                            .iter()
                            .find(|i| i.item_id == item_ref)
                            .map(|i| i.name.clone())
                            .or_else(|| {
                                ws.nearby_items
                                    .iter()
                                    .find(|i| i.item_id == item_ref)
                                    .map(|i| i.name.clone())
                            })
                            .or_else(|| {
                                ws.location
                                    .gatherable_items
                                    .iter()
                                    .find(|g| g.item_id == item_ref)
                                    .map(|g| g.name.clone())
                            })
                    });
                    match item_name {
                        Some(name) => config
                            .high_value_item_keywords
                            .iter()
                            .any(|k| name.contains(k.as_str())),
                        // 不可见引用：交由 layer0 拒绝；此处保守升级 LLM
                        None => true,
                    }
                }
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
        if intent.action_type.as_str() == "休整" {
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

        // 查找匹配的 action 定义：精确匹配 action/name，或词表条目规范化后与
        // intent（已规范化）一致——兼容新旧两套词表并存过渡期（不做任意 alias 匹配）
        let action_input = intent.action_type.as_str().to_lowercase();
        let matched = actions.iter().find(|a| {
            a.action == intent.action_type.as_str()
                || a.name.to_lowercase() == action_input
                || cyber_jianghu_protocol::normalize_action_type(&a.action, &mut None)
                    .to_lowercase()
                    == action_input
                || cyber_jianghu_protocol::normalize_action_type(&a.name, &mut None).to_lowercase()
                    == action_input
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
                    return Err(format!("动作 '{}' 缺少必需字段: {}", action.name, field));
                }
            }
            // 对话类动作 content 占位符检测：LLM 偶尔输出 "..." 替代实际对话内容，
            // 导致前端经历日志显示省略号而非文字，此处拦截并要求重新生成
            if intent.action_type.as_str() == "说话"
                && let Some(content) = intent
                    .action_data
                    .as_ref()
                    .and_then(|d| d.get("content"))
                    .and_then(|v| v.as_str())
            {
                let trimmed = content.trim();
                if trimmed.is_empty()
                    || matches!(
                        trimmed,
                        "..." | "…" | "。。。" | ".." | "。" | "-" | "--" | "---"
                    )
                {
                    return Err(format!(
                        "动作 '{}' 的 content 不能为空或占位符，请写出实际的对话内容",
                        action.name
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
            .unwrap_or("休整");

        Err(format!(
            "action '{}' 不在合法列表中，合法值: [{}]，最接近: '{}'",
            intent.action_type,
            valid_names.join(", "),
            suggestion,
        ))
    }

    /// Layer 2：RuleEngine 规则校验
    ///
    /// 目标可见性硬性校验已前置到 Layer 0（validate_hard_logic）
    async fn validate_with_rule_engine(
        &self,
        request: &ValidationRequest,
    ) -> std::result::Result<(), String> {
        let Some(world_state) = request.world_state.as_ref() else {
            return Ok(());
        };

        let (available_item_ids, reachable_node_ids) = extract_ids_from_world_state(world_state);
        let context = RuleValidationContext {
            intent: request.intent.clone(),
            persona_info: request.persona.clone(),
            world_context: request.world_context.clone(),
            tick_id: world_state.tick_id,
            attributes: std::collections::HashMap::new(),
            available_item_ids,
            reachable_node_ids,
        };

        match self.rule_engine.validate_context(&context).await {
            Ok(ValidationResult::Approved { .. }) => Ok(()),
            Ok(ValidationResult::Rejected { reason, .. }) => Err(reason),
            Err(e) => {
                tracing::warn!("RuleEngine error, rejecting: {}", e);
                Err(format!("RuleEngine 内部错误: {}", e))
            }
        }
    }

    /// Layer 1/2/3 统一出口
    pub async fn validate_pipeline(
        &self,
        request: ValidationRequest,
    ) -> Result<PipelineValidationResult> {
        let mut request = request;

        // 动作名规范化（全面 uuid 化配套）：LLM 语义/英文别名（idle/进食/给予/采集…）
        // → canonical 键 + 缺省字段注入。必须在 layer0 之前执行，
        // 使后续所有审查层与 Server 看到统一的动作词表。
        {
            let normalized = cyber_jianghu_protocol::normalize_action_type(
                request.intent.action_type.as_str(),
                &mut request.intent.action_data,
            );
            request.intent.action_type = normalized.into();
            for si in request.intent.subsequent_intents.iter_mut() {
                let normalized = cyber_jianghu_protocol::normalize_action_type(
                    si.action_type.as_str(),
                    &mut si.action_data,
                );
                si.action_type = normalized.into();
            }
        }

        let graded_config = request.runtime.graded_config.clone();
        let mut layers = Vec::with_capacity(4);

        // Layer 0：硬性逻辑审查（目标可见性，拦截 LLM 臆测目标）
        match super::hard_logic::validate_hard_targets(&mut request, &self.rules).await {
            Ok(()) => layers.push(LayerResult {
                layer: "layer0",
                passed: true,
                detail: None,
            }),
            Err(reason) => {
                layers.push(LayerResult {
                    layer: "layer0",
                    passed: false,
                    detail: Some(reason.clone()),
                });
                return Ok(PipelineValidationResult::Rejected { reason, layers });
            }
        }

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

        match self.validate_with_rule_engine(&request).await {
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
            || Self::should_skip_llm_validation(
                &request.intent,
                graded_config.as_ref(),
                request.world_state.as_ref(),
            )
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
                let reason = format!("LLM error, rejecting: {}", e);
                layers.push(LayerResult {
                    layer: "layer3",
                    passed: false,
                    detail: Some(reason.clone()),
                });
                return Ok(PipelineValidationResult::Rejected { reason, layers });
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
                    let has_survival_drive = world_state
                        .self_state
                        .survival_drives
                        .iter()
                        .any(|sd| sd.attribute == "satiation" || sd.attribute == "hydration");
                    if has_survival_drive {
                        layers.push(LayerResult {
                            layer: "layer3",
                            passed: true,
                            detail: Some(format!(
                                "survival_override: drives={:?}",
                                world_state
                                    .self_state
                                    .survival_drives
                                    .iter()
                                    .map(|sd| format!("{}({})", sd.attribute, sd.urgency))
                                    .collect::<Vec<_>>()
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

        let recent_decisions = &request.runtime.recent_same_type_decisions;
        let recent_ref = if recent_decisions.is_empty() {
            None
        } else {
            Some(recent_decisions.as_slice())
        };

        // 构建验证 prompt（条件注入语义去重指令）
        let prompt = self.reflector_prompt.build_validation_prompt(
            &request.intent,
            &request.persona,
            &rules,
            &request.world_context,
            recent_ref,
        );

        debug!("Validation prompt:\n{}", prompt);

        // 调用 LLM（从 container 读取当前客户端，支持热重载）
        let llm_client = self.llm_container.read().await.clone();
        let chat_config = crate::component::llm::ChatExchangeConfig {
            model: llm_client.model_name(),
            temperature: llm_client.temperature(),
            max_tokens: None,
            enable_thinking: None,
        };
        let extracted = llm_client
            .complete_json_with_system_and_retry_extracted(
                self.reflector_prompt.system_prompt(),
                &prompt,
                chat_config,
                2,
            )
            .await?;
        let response: LlmValidationResponse = extracted.value;

        thinking_log::log_llm(
            &format!("Agent({})", request.intent.agent_id),
            request.intent.tick_id,
            "ReflectorSoul",
            &prompt,
            &format!("{:?}", response),
        );

        // 训练 trace（天魂审查路径）
        crate::infra::api::trace::record(crate::infra::api::trace::LlmTrace {
            trace_id: uuid::Uuid::new_v4().to_string(),
            agent_id: request.intent.agent_id,
            character_name: format!("Agent({})", request.intent.agent_id),
            tick_id: request.intent.tick_id,
            soul_stage: crate::infra::api::trace::SoulStage::Tianhun,
            attempt: 0,
            provider: llm_client.provider_name(),
            model: llm_client.model_name(),
            persona_name: String::new(),
            persona_description: String::new(), // 天魂审查无 persona 上下文
            user_prompt: prompt.clone(),
            response: format!("{:?}", response),
            prompt_tokens: None,
            completion_tokens: None,
            ok: true,
            wall_clock: chrono::Utc::now(),
        });

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

        // 提取 rules_json 后存入
        let rules_json = rules.rules_json.clone();
        *current_rules = rules;
        info!(
            "WorldBuildingRules updated to version {}",
            current_rules.version
        );

        // 释放写锁后更新 RuleEngine
        drop(current_rules);
        if let Some(json) = rules_json {
            self.rule_engine.reload_rules_from_json(json).await;
        }
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
        let chat_config = crate::component::llm::ChatExchangeConfig {
            model: llm_client.model_name(),
            temperature: llm_client.temperature(),
            max_tokens: None,
            enable_thinking: None,
        };
        let extracted = llm_client
            .complete_json_with_system_and_retry_extracted(
                self.reflector_prompt.system_prompt(),
                &prompt,
                chat_config,
                2,
            )
            .await?;
        let response: LlmValidationResponse = extracted.value;

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
#[path = "validator_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "hard_logic_tests.rs"]
mod tests_layer0;
