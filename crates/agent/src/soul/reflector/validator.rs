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

    /// 基于上轮 submitted intents + server ExecutionSummary 生成叙事化经历
    ///
    /// `first_tick` 为 true 时表示这是本轮首次生成叙事（无历史数据）。
    /// 默认返回 None（不生成叙事）。
    async fn generate_execution_narrative(
        &self,
        last_intents: &[crate::models::Intent],
        execution_summary: &cyber_jianghu_protocol::ExecutionSummary,
        first_tick: bool,
    ) -> Result<Option<String>> {
        let _ = (last_intents, execution_summary, first_tick);
        Ok(None)
    }
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

    async fn generate_execution_narrative(
        &self,
        last_intents: &[crate::models::Intent],
        execution_summary: &cyber_jianghu_protocol::ExecutionSummary,
        first_tick: bool,
    ) -> Result<Option<String>> {
        self.generate_execution_narrative_impl(last_intents, execution_summary, first_tick)
            .await
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

    /// 基于上轮 submitted intents + server ExecutionSummary 生成叙事化经历
    ///
    /// 在收到 server WorldState（包含 last_execution_summary）后调用。
    /// 生成「上轮你做了什么，结果如何」的第一人称叙事。
    ///
    /// `first_tick` 为 true 时表示这是本轮首次生成叙事（无历史数据），应生成「初入江湖」类叙事。
    async fn generate_execution_narrative_impl(
        &self,
        last_intents: &[crate::models::Intent],
        execution_summary: &cyber_jianghu_protocol::ExecutionSummary,
        first_tick: bool,
    ) -> Result<Option<String>> {
        // 构建 intents 描述（空列表也能正常处理）
        let intents_desc: Vec<String> = last_intents
            .iter()
            .map(|i| {
                let data_str = i
                    .action_data
                    .as_ref()
                    .map(|d| serde_json::to_string(d).unwrap_or_default())
                    .unwrap_or_default();
                format!("- {}: {}", i.action_type, data_str)
            })
            .collect();

        // 根据是否首 tick 选择不同的 prompt
        let prompt = if first_tick {
            // 首 tick：生成「初入江湖」类叙事
            format!(
                r#"你是叙事生成器。这是本轮首次叙事，生成一段简短的第一人称叙事，描述「你踏入江湖的第一感受」。

## 上轮提交的意图
{}

## 执行结果
- 总计: {} 个意图
- 成功: {}
- 部分成功: {}
- 失败: {}
- 跳过: {}

## 要求
1. 用武侠风格的第一人称叙事
2. 不要提及具体数字或百分比
3. 不要提及「意图」「执行」「成功」等游戏术语
4. 生成「初入江湖，踌躇满志」类叙事
5. 简洁，一段话即可

## 输出格式
直接输出叙事文本，不要加引号或任何格式标记。"#,
                intents_desc.join("\n"),
                execution_summary.total,
                execution_summary.succeeded,
                execution_summary.partial,
                execution_summary.failed,
                execution_summary.skipped
            )
        } else {
            // 非首 tick：生成上轮经历叙事
            format!(
                r#"你是叙事生成器。基于以下上轮意图和执行结果，生成一段简短的第一人称叙事，描述「上轮你做了什么，结果如何」。

## 上轮提交的意图
{}

## 执行结果
- 总计: {} 个意图
- 成功: {}
- 部分成功: {}
- 失败: {}
- 跳过: {}

## 要求
1. 用武侠风格的第一人称叙事（如「我拾起了馒头」「我喝了几口水」）
2. 不要提及具体数字或百分比
3. 不要提及「意图」「执行」「成功」等游戏术语
4. 生成的叙事应该让人魂能够理解上轮行动的效果
5. 如果所有意图都失败或跳过，生成「似乎什么都没发生」类的叙事
6. 简洁，一段话即可

## 输出格式
直接输出叙事文本，不要加引号或任何格式标记。"#,
                intents_desc.join("\n"),
                execution_summary.total,
                execution_summary.succeeded,
                execution_summary.partial,
                execution_summary.failed,
                execution_summary.skipped
            )
        };

        // 重试机制：最多 2 次
        let max_retries = 2;
        for attempt in 1..=max_retries {
            let llm_client = self.llm_container.read().await.clone();
            match llm_client.complete(&prompt).await {
                Ok(n) => {
                    let narrative = n.trim().to_string();
                    if !narrative.is_empty() {
                        return Ok(Some(narrative));
                    }
                    warn!(
                        "天魂生成执行叙事返回空 (attempt {}/{})",
                        attempt, max_retries
                    );
                }
                Err(e) => {
                    warn!(
                        "天魂生成执行叙事失败 (attempt {}/{}): {}",
                        attempt, max_retries, e
                    );
                }
            }
            // 空响应或错误，继续重试
        }

        // 所有重试都失败
        warn!("天魂生成执行叙事重试耗尽，跳过本轮叙事");
        Ok(None)
    }
}

// ============================================================================
// 分级审核辅助
// ============================================================================

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
