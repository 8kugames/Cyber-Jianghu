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
}
