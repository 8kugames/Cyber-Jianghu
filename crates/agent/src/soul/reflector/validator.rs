// ============================================================================
// 意图验证引擎
// ============================================================================

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::component::llm::{LlmClient, LlmClientExt};
use crate::infra::api::thinking_log;
use cyber_jianghu_protocol::WorldBuildingRules;

use super::prompt::ObserverPrompt;
use super::types::{LlmValidationResponse, PersonaInfo, ValidationRequest, ValidationResult};

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

/// 为 IntentValidator 实现 Validator trait
#[async_trait]
impl Validator for IntentValidator {
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

/// 意图验证引擎
pub struct IntentValidator {
    /// 世界观规则
    rules: Arc<RwLock<WorldBuildingRules>>,
    /// LLM 客户端（注入的外部实现）
    llm_client: Arc<dyn LlmClient>,
    /// 观察者 prompt 模板
    observer_prompt: ObserverPrompt,
}

impl IntentValidator {
    /// 创建新的验证器
    pub fn new(rules: WorldBuildingRules, llm_client: Arc<dyn LlmClient>) -> Self {
        Self {
            rules: Arc::new(RwLock::new(rules)),
            llm_client,
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

        // 调用 LLM（system + user 分离，利用 system message 优先级）
        let response: LlmValidationResponse = self
            .llm_client
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

        let response: LlmValidationResponse = self
            .llm_client
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
                    "Unrecognized validation result '{}', auto-approving (lenient policy)",
                    self.result
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

    #[tokio::test]
    async fn test_validate_approved() {
        let mock_client = MockLlmClient::with_response(
            r#"{
            "result": "approved",
            "reason": "行为符合武侠世界观",
            "narrative": "李四决定在客栈休息"
        }"#,
        );

        let validator = IntentValidator::new(WorldBuildingRules::default(), Arc::new(mock_client));

        let request = ValidationRequest {
            intent: crate::models::Intent::new(uuid::Uuid::new_v4(), 1, "idle", None),
            persona: PersonaInfo::default(),
            world_context: "龙门客栈".to_string(),
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

        let validator = IntentValidator::new(WorldBuildingRules::default(), Arc::new(mock_client));

        let request = ValidationRequest {
            intent: crate::models::Intent::new(uuid::Uuid::new_v4(), 1, "idle", None),
            persona: PersonaInfo::default(),
            world_context: "龙门客栈".to_string(),
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

        let validator = IntentValidator::new(WorldBuildingRules::default(), Arc::new(mock_client));

        // Test that update_rules doesn't panic
        let new_rules = WorldBuildingRules::default();
        validator.update_rules(new_rules).await;
    }
}
