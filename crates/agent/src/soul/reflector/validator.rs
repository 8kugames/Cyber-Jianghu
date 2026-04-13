// ============================================================================
// ReflectorSoul — 意图审查引擎
// ============================================================================

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::component::llm::LlmClientExt;
use crate::infra::api::thinking_log;
use crate::runtime::claw::LlmClientContainer;
use cyber_jianghu_protocol::WorldBuildingRules;

use super::prompt::ObserverPrompt;
use super::types::{
    BatchValidationResult, LlmValidationResponse, PersonaInfo, RejectionReason, ValidationRequest,
    ValidationResult,
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
    async fn validate(&self, request: ValidationRequest) -> Result<ValidationResult>;

    /// 验证人设
    async fn validate_persona(&self, persona: &PersonaInfo) -> Result<ValidationResult>;

    /// 更新世界观规则
    async fn update_rules(&self, rules: WorldBuildingRules);
}

/// 为 ReflectorSoul 实现 Validator trait
#[async_trait]
impl Validator for ReflectorSoul {
    async fn validate(&self, request: ValidationRequest) -> Result<ValidationResult> {
        self.validate(request).await
    }

    async fn validate_persona(&self, persona: &PersonaInfo) -> Result<ValidationResult> {
        self.validate_persona(persona).await
    }

    async fn update_rules(&self, rules: WorldBuildingRules) {
        self.update_rules(rules).await
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
}

impl ReflectorSoul {
    /// 创建新的 ReflectorSoul
    pub fn new(rules: WorldBuildingRules, llm_container: LlmClientContainer) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
            llm_container,
            observer_prompt: ObserverPrompt::default(),
        }
    }

    /// 验证意图
    ///
    /// 返回包含叙事的结果，避免额外 LLM 调用
    pub async fn validate(&self, request: ValidationRequest) -> Result<ValidationResult> {
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

- 性别：{}
- 年龄：{}
- 性格：{}
- 价值观：{}

注意：人设中的性格和价值观不应包含现代概念、魔法元素或穿越者知识。

请按以下 JSON 格式输出：
{{
  "result": "approved" | "rejected",
  "reason": "通过/驳回的原因",
  "rejection_type": "era_violation" | "other",
  "narrative": "如果是 approved，生成一段简短的人设描述"
}}"#,
            rules.era.name,
            rules.era.tech_level,
            rules.forbidden_concepts.join("、"),
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

    /// 批次验证多 Intent（分级审核策略）
    ///
    /// 根据配置的 action_type 分类决定 LLM 审核级别：
    /// - Always: 强制 LLM 审核（speak, shout, whisper）
    /// - Adaptive: 根据 action_data 动态判断
    /// - Skip: 跳过 LLM，只做 RuleEngine
    pub async fn validate_batch(
        &self,
        intents: Vec<crate::models::Intent>,
        persona: &PersonaInfo,
        world_context: &str,
        world_state: Option<&cyber_jianghu_protocol::WorldState>,
        graded_config: Option<&cyber_jianghu_protocol::GradedValidationConfig>,
    ) -> BatchValidationResult {
        let config = graded_config.cloned().unwrap_or_default();
        let mut valid_intents = Vec::new();
        let mut rejections = Vec::new();
        let mut llm_count = 0;

        for intent in intents {
            let level = Self::determine_validation_level(&intent, &config);
            let force_llm = match level {
                ValidationLevel::Always => true,
                ValidationLevel::Skip => false,
                ValidationLevel::Adaptive => Self::adaptive_check(&intent, &config),
            };

            if force_llm {
                llm_count += 1;
            }

            let request = ValidationRequest {
                intent: intent.clone(),
                persona: persona.clone(),
                world_context: world_context.to_string(),
                world_state: world_state.cloned(),
            };

            match self.validate(request).await {
                Ok(ValidationResult::Approved { .. }) => {
                    valid_intents.push(intent);
                }
                Ok(ValidationResult::Rejected {
                    reason,
                    rejection_type,
                }) => {
                    let intent_id = intent.intent_id;
                    rejections.push((
                        intent,
                        RejectionReason {
                            intent_id,
                            reason,
                            rejection_type,
                        },
                    ));
                }
                Err(e) => {
                    // LLM 调用失败 → 宽松策略：直接通过（只做 RuleEngine）
                    warn!("验证LLM调用失败，宽松通过: {}", e);
                    valid_intents.push(intent);
                }
            }
        }

        // 确保每 tick 至少 minimum_per_tick 个 Intent 经过 LLM 审查
        // 智能选择：优先高风险（Always > Adaptive > Skip），而非随机
        if llm_count < config.minimum_per_tick && !valid_intents.is_empty() {
            let idx = self.select_highest_risk_intent_index(&valid_intents, &config);
            let intent = valid_intents.remove(idx);
            let intent_id = intent.intent_id;
            let request = ValidationRequest {
                intent: intent.clone(),
                persona: persona.clone(),
                world_context: world_context.to_string(),
                world_state: world_state.cloned(),
            };
            match self.validate(request).await {
                Ok(ValidationResult::Approved { .. }) => {
                    valid_intents.insert(idx, intent);
                }
                Ok(ValidationResult::Rejected {
                    reason,
                    rejection_type,
                }) => {
                    rejections.push((
                        intent,
                        RejectionReason {
                            intent_id,
                            reason,
                            rejection_type,
                        },
                    ));
                }
                Err(_) => {
                    valid_intents.insert(idx, intent);
                }
            }
        }

        BatchValidationResult {
            valid_intents,
            rejections,
        }
    }

    /// 确定审核级别
    fn determine_validation_level(
        intent: &crate::models::Intent,
        config: &cyber_jianghu_protocol::GradedValidationConfig,
    ) -> ValidationLevel {
        let action_str = intent.action_type.as_str().to_string();
        if config.always_types.contains(&action_str) {
            ValidationLevel::Always
        } else if config.skip_types.contains(&action_str) {
            ValidationLevel::Skip
        } else if config.adaptive_types.contains(&action_str) {
            ValidationLevel::Adaptive
        } else {
            ValidationLevel::Skip
        }
    }

    /// Adaptive 检查：根据 action_data 判断是否需要 LLM
    fn adaptive_check(
        intent: &crate::models::Intent,
        config: &cyber_jianghu_protocol::GradedValidationConfig,
    ) -> bool {
        let action_data = intent.action_data.as_ref();
        match intent.action_type.as_str() {
            "move" => {
                action_data.is_some_and(|d| is_restricted_area(d, &config.restricted_area_keywords))
            }
            "trade" | "steal" | "give" => action_data
                .is_some_and(|d| is_high_value_transaction(d, &config.high_value_item_keywords)),
            _ => true,
        }
    }

    /// 选择最高风险的 Intent 索引（用于 minimum_per_tick 智能选择）
    ///
    /// 优先级：Always (高风险) > Adaptive (中风险) > Skip (低风险)
    /// 同级别保持原始顺序（stable）
    fn select_highest_risk_intent_index(
        &self,
        intents: &[crate::models::Intent],
        config: &cyber_jianghu_protocol::GradedValidationConfig,
    ) -> usize {
        intents
            .iter()
            .enumerate()
            .max_by_key(|(idx, intent)| {
                let level = Self::determine_validation_level(intent, config);
                // 优先级分数：Always=2, Adaptive=1, Skip=0
                let priority = match level {
                    ValidationLevel::Always => 2,
                    ValidationLevel::Adaptive => 1,
                    ValidationLevel::Skip => 0,
                };
                // 使用 (priority, idx的反序) 作为排序键
                // 这样同级别时保持原始顺序（较小的 idx 优先）
                (priority, std::cmp::Reverse(*idx))
            })
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }
}

// ============================================================================
// 分级审核辅助
// ============================================================================

/// 审核级别
enum ValidationLevel {
    /// 高风险：100% LLM 审核
    Always,
    /// 中风险：根据 action_data 动态判断
    Adaptive,
    /// 低风险：跳过 LLM
    Skip,
}

/// 检查是否限制区域（配置驱动）
fn is_restricted_area(action_data: &serde_json::Value, keywords: &[String]) -> bool {
    action_data
        .get("target_location")
        .and_then(|v| v.as_str())
        .map(|loc| keywords.iter().any(|r| loc.contains(r.as_str())))
        .unwrap_or(false)
}

/// 检查是否高价值交易（配置驱动）
fn is_high_value_transaction(action_data: &serde_json::Value, keywords: &[String]) -> bool {
    action_data
        .get("item_id")
        .and_then(|v| v.as_str())
        .map(|id| keywords.iter().any(|v| id.contains(v.as_str())))
        .unwrap_or(false)
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
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn mock_container(client: MockLlmClient) -> LlmClientContainer {
        Arc::new(RwLock::new(Arc::new(client)))
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
            ReflectorSoul::new(WorldBuildingRules::default(), mock_container(mock_client));

        let request = ValidationRequest {
            intent: crate::models::Intent::new(uuid::Uuid::new_v4(), 1, "idle", None),
            persona: PersonaInfo::default(),
            world_context: "龙门客栈".to_string(),
            world_state: None,
        };

        let result = validator.validate(request).await.unwrap();

        match result {
            ValidationResult::Approved { reason, narrative } => {
                assert_eq!(reason, Some("行为符合武侠世界观".to_string()));
                assert_eq!(narrative, "李四决定在客栈休息");
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
            ReflectorSoul::new(WorldBuildingRules::default(), mock_container(mock_client));

        let request = ValidationRequest {
            intent: crate::models::Intent::new(uuid::Uuid::new_v4(), 1, "idle", None),
            persona: PersonaInfo::default(),
            world_context: "龙门客栈".to_string(),
            world_state: None,
        };

        let result = validator.validate(request).await.unwrap();

        match result {
            ValidationResult::Rejected {
                reason,
                rejection_type,
            } => {
                assert_eq!(reason, "使用了魔法，违反力量体系");
                assert_eq!(
                    rejection_type,
                    super::super::types::RejectionType::PowerSystemViolation
                );
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
            ReflectorSoul::new(WorldBuildingRules::default(), mock_container(mock_client));

        // Test that update_rules doesn't panic
        let new_rules = WorldBuildingRules::default();
        validator.update_rules(new_rules).await;
    }
}
