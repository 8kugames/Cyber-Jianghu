// ============================================================================
// 认知验证器 (Cognitive Validator - Gatekeeper)
// ============================================================================
//
// 验证认知链结构完整性，确保 LLM 输出了有效的内容。
//
// 注意：本验证器是确定性规则引擎，不调用 LLM。
// 与 RuleEngine 的区别：RuleEngine 验证意图合规性（冷却/资源），
// CognitiveValidator 验证认知链的结构完整性（阶段齐全/内容非空/无重复）。
//
// 接入点：decision.rs 的 cognitive_decision_with_retry() 重试循环
//
// 验证规则：
// - R1 完整性：4 个阶段必须齐全
// - R2 长度：每阶段内容 >= min_thought_length（默认 10 字符）
// ============================================================================

use crate::soul::actor::{CognitiveChain, CognitiveStage};

/// 认知验证结果
#[derive(Debug, Clone)]
pub struct CognitiveValidationResult {
    /// 是否通过验证
    pub is_valid: bool,
    /// 验证失败的原因（如果失败）
    pub reason: Option<String>,
    /// 改进建议（如果失败）
    pub suggestion: Option<String>,
}

impl CognitiveValidationResult {
    /// 创建通过结果
    pub fn approved() -> Self {
        Self {
            is_valid: true,
            reason: None,
            suggestion: None,
        }
    }

    /// 创建拒绝结果
    pub fn rejected(reason: String, suggestion: String) -> Self {
        Self {
            is_valid: false,
            reason: Some(reason),
            suggestion: Some(suggestion),
        }
    }
}

/// 认知验证器 (Gatekeeper)
///
/// 验证认知链结构完整性
pub struct CognitiveValidator {
    /// 最小思考长度阈值
    min_thought_length: usize,
}

impl CognitiveValidator {
    /// 创建新的认知验证器
    pub fn new(_agent_persona: String) -> Self {
        Self {
            min_thought_length: 10,
        }
    }

    /// 设置最小思考长度阈值
    pub fn with_min_thought_length(mut self, length: usize) -> Self {
        self.min_thought_length = length;
        self
    }

    /// 验证认知链质量
    pub fn validate(&self, chain: &CognitiveChain) -> CognitiveValidationResult {
        // 规则 1: 检查认知链是否完整
        if let Some(result) = self.check_completeness(chain) {
            return result;
        }

        // 规则 2: 检查各阶段内容长度
        if let Some(result) = self.check_content_length(chain) {
            return result;
        }

        CognitiveValidationResult::approved()
    }

    // ========================================================================
    // 验证规则实现
    // ========================================================================

    /// 规则 1: 检查认知链是否完整
    fn check_completeness(&self, chain: &CognitiveChain) -> Option<CognitiveValidationResult> {
        let expected_stages = CognitiveStage::all();

        if chain.stages.len() != expected_stages.len() {
            return Some(CognitiveValidationResult::rejected(
                format!(
                    "认知链不完整，需要 {} 个阶段，实际只有 {} 个",
                    expected_stages.len(),
                    chain.stages.len()
                ),
                "请确保完成所有认知阶段：感知→动机→规划→决策".to_string(),
            ));
        }

        // 检查是否每个阶段都存在
        for expected_stage in &expected_stages {
            if !chain.stages.iter().any(|s| &s.stage == expected_stage) {
                return Some(CognitiveValidationResult::rejected(
                    format!("缺少 {} 阶段", expected_stage.name()),
                    format!("请补充 {} 阶段的思考", expected_stage.name()),
                ));
            }
        }

        None
    }

    /// 规则 2: 检查各阶段内容长度
    fn check_content_length(&self, chain: &CognitiveChain) -> Option<CognitiveValidationResult> {
        for stage_output in &chain.stages {
            let content_len = stage_output.content.trim().len();

            if content_len < self.min_thought_length {
                return Some(CognitiveValidationResult::rejected(
                    format!(
                        "{} 阶段内容过短 ({} 字符)",
                        stage_output.stage.name(),
                        content_len
                    ),
                    format!(
                        "{} 阶段至少需要 {} 字符的思考内容",
                        stage_output.stage.name(),
                        self.min_thought_length
                    ),
                ));
            }
        }

        None
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::CognitiveStage;
    use uuid::Uuid;

    fn create_test_chain() -> crate::soul::actor::CognitiveChain {
        let mut chain = crate::soul::actor::CognitiveChain::new(
            "测试侠客".to_string(),
            "测试人设".to_string(),
            1,
        );

        chain.add_stage(crate::core::StageOutput::new(
            CognitiveStage::Perception,
            "HP: 50, 饥饿: 20, 体力: 80".to_string(),
        ));

        chain.add_stage(crate::core::StageOutput::new(
            CognitiveStage::Motivation,
            "我需要食物，因为饥饿值很低".to_string(),
        ));

        chain.add_stage(crate::core::StageOutput::new(
            CognitiveStage::Planning,
            "计划: 1. 找食物 2. 吃掉".to_string(),
        ));

        chain.add_stage(crate::core::StageOutput::new(
            CognitiveStage::Decision,
            "因为饥饿，所以决定使用馒头".to_string(),
        ));

        // 设置 final_intent
        chain.final_intent = crate::models::Intent::new(
            Uuid::new_v4(),
            1,
            "使用",
            Some(serde_json::json!({"item_id": "mantou"})),
        )
        .with_thought("因为饥饿，所以决定使用馒头".to_string());

        chain
    }

    #[test]
    fn test_validator_approves_valid_chain() {
        let validator = CognitiveValidator::new("测试人设".to_string());
        let chain = create_test_chain();

        let result = validator.validate(&chain);
        assert!(result.is_valid, "Valid chain should pass validation");
    }

    #[test]
    fn test_validator_rejects_incomplete_chain() {
        let validator = CognitiveValidator::new("测试人设".to_string());
        let mut chain = crate::soul::actor::CognitiveChain::new(
            "测试侠客".to_string(),
            "测试人设".to_string(),
            1,
        );

        // 只添加一个阶段
        chain.add_stage(crate::core::StageOutput::new(
            CognitiveStage::Perception,
            "HP: 50".to_string(),
        ));

        let result = validator.validate(&chain);
        assert!(!result.is_valid, "Incomplete chain should fail validation");
        assert!(result.reason.unwrap().contains("不完整"));
    }

    #[test]
    fn test_validator_rejects_short_content() {
        let validator = CognitiveValidator::new("测试人设".to_string()).with_min_thought_length(10);

        let mut chain = crate::soul::actor::CognitiveChain::new(
            "测试侠客".to_string(),
            "测试人设".to_string(),
            1,
        );

        // 添加所有阶段，但内容太短
        for stage in CognitiveStage::all() {
            chain.add_stage(crate::core::StageOutput::new(stage, "短".to_string()));
        }

        let result = validator.validate(&chain);
        assert!(!result.is_valid, "Chain with short content should fail");
        assert!(result.reason.unwrap().contains("过短"));
    }
}
