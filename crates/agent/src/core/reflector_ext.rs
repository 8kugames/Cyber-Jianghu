// ============================================================================
// ReflectorSoul 审查扩展
// ============================================================================
//
// Agent 的天魂审查相关方法。
// 三层审查本体已回收至 ReflectorSoul，本文件只保留 Agent 侧编排与反馈。
// ============================================================================

use anyhow::Result;
use cyber_jianghu_protocol::WorldState;

/// 人设验证结果
#[derive(Debug)]
pub enum PersonaValidationResult {
    /// 验证通过
    Approved,
    /// 需要修改
    NeedsRevision {
        reason: String,
        rejection_type: crate::soul::reflector::RejectionType,
    },
    /// 跳过验证（无验证器）
    Skipped,
}

impl super::Agent {
    pub(crate) fn set_rejection_feedback(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        self.last_rejection_reason = Some(Self::narrativize_rejection(&reason));
    }

    pub(crate) async fn validate_with_reflector(
        &mut self,
        intent: cyber_jianghu_protocol::Intent,
        world_state: &WorldState,
        graded_config: Option<&cyber_jianghu_protocol::GradedValidationConfig>,
    ) -> Result<crate::soul::reflector::PipelineValidationResult> {
        let Some(validator) = &self.validator else {
            return Ok(crate::soul::reflector::PipelineValidationResult::Approved {
                intent,
                layers: vec![],
                narrative: None,
            });
        };

        let request = crate::soul::reflector::ValidationRequest {
            intent,
            persona: self.extract_persona(),
            world_context: self.build_world_context(world_state),
            world_state: Some(world_state.clone()),
            runtime: crate::soul::reflector::ValidationRuntimeConfig {
                graded_config: graded_config.cloned(),
                consecutive_follow_count: self.consecutive_follow_count as usize,
                max_consecutive_follow: self.config.llm.max_consecutive_follow,
            },
        };

        validator.validate(request).await
    }

    // ========================================================================
    // 人设验证
    // ========================================================================

    /// 验证人设合规性
    pub async fn validate_persona(&self) -> Result<PersonaValidationResult> {
        let validator = match &self.validator {
            Some(v) => v,
            None => return Ok(PersonaValidationResult::Skipped),
        };

        let persona = self.extract_persona();

        match validator.validate_persona(&persona).await? {
            crate::soul::reflector::ValidationResult::Approved { .. } => {
                Ok(PersonaValidationResult::Approved)
            }
            crate::soul::reflector::ValidationResult::Rejected {
                reason,
                rejection_type,
            } => Ok(PersonaValidationResult::NeedsRevision {
                reason,
                rejection_type,
            }),
        }
    }

    // ========================================================================
    // Self-Correction（优化模式：被驳回后调用 LLM 纠正一次）
    // ========================================================================

    /// 基于驳回原因调用 LLM 生成纠正后的 Intent
    ///
    /// 复用 decision_with_chain_callback 基础设施：
    /// 1. 设置 last_rejection_feedback（callback 会读取并传给 LLM）
    /// 2. 调用 callback 生成纠正后的 intent
    pub(crate) async fn self_correct_intent(
        &mut self,
        world_state: &WorldState,
        memory_context: &str,
        rejection_reason: &str,
    ) -> Result<cyber_jianghu_protocol::Intent> {
        // 设置驳回反馈，使 callback 能传递给 LLM
        self.set_rejection_feedback(rejection_reason.to_string());

        let tick_id = world_state.tick_id;
        let agent_id = world_state.agent_id.unwrap_or_default();

        // 优先使用 decision_with_chain_callback
        if let Some(ref chain_callback) = self.decision_with_chain_callback {
            let fb = self.last_rejection_reason.as_deref();
            let (corrected_intent, _) = chain_callback(world_state, memory_context, fb).await;
            return Ok(corrected_intent);
        }

        // 降级路径：旧式回调
        if let Some(ref callback) = self.decision_with_feedback_callback {
            let intent = callback(tick_id, agent_id, memory_context, Some(rejection_reason)).await;
            return Ok(intent);
        }

        if let Some(ref memory_callback) = self.decision_with_memory_callback {
            let combined = format!(
                "{}\n[意图被驳回: {}，请纠正并重新决策]",
                memory_context, rejection_reason
            );
            let intent = memory_callback(tick_id, agent_id, &combined).await;
            return Ok(intent);
        }

        // 最终降级：基础 callback（无反馈能力）
        let intent = (self.decision_callback)(tick_id, agent_id).await;
        Ok(intent)
    }
}
