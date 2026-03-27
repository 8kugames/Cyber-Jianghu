// ============================================================================
// 认知验证器 (Cognitive Validator - Gatekeeper)
// ============================================================================
//
// 验证认知链质量，拒绝"偷懒"行为，强制深度思考
//
// 核心设计：
// - 检查各阶段输出是否完整
// - 验证是否有对 WorldState 的引用
// - 检测重复模式和"复制粘贴"行为
// - 确保推理连贯性
// ============================================================================

use crate::core::cognitive::{CognitiveChain, CognitiveStage};
use anyhow::Result;

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
/// 验证认知链质量，确保 LLM 进行了深度思考
#[allow(dead_code)]
pub struct CognitiveValidator {
    /// Agent 人设（预留：基于人设的验证逻辑）
    agent_persona: String,
    /// 最小思考长度阈值
    min_thought_length: usize,
    /// 是否启用严格模式
    strict_mode: bool,
}

impl CognitiveValidator {
    /// 创建新的认知验证器
    pub fn new(agent_persona: String) -> Self {
        Self {
            agent_persona,
            min_thought_length: 20,
            strict_mode: true,
        }
    }

    /// 设置最小思考长度阈值
    pub fn with_min_thought_length(mut self, length: usize) -> Self {
        self.min_thought_length = length;
        self
    }

    /// 设置是否启用严格模式
    pub fn with_strict_mode(mut self, strict: bool) -> Self {
        self.strict_mode = strict;
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

        // 规则 3: 检查是否有对 WorldState 的引用
        if let Some(result) = self.check_state_reference(chain) {
            return result;
        }

        // 规则 4: 检测重复模式
        if let Some(result) = self.detect_repetition(chain) {
            return result;
        }

        // 规则 5: 检查推理连贯性
        if let Some(result) = self.check_coherence(chain) {
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

    /// 规则 3: 检查是否有对 WorldState 的引用
    fn check_state_reference(&self, chain: &CognitiveChain) -> Option<CognitiveValidationResult> {
        // 检查感知阶段是否引用了状态关键词
        let perception = chain.get_stage(CognitiveStage::Perception)?;

        // 关键词列表：应该出现在感知中的词
        let state_keywords = [
            "hp", "HP", "生命", "hunger", "饥饿", "thirst", "口渴", "stamina", "体力", "背包",
            "物品", "附近", "周围", "位置", "地点",
        ];

        let perception_lower = perception.content.to_lowercase();
        let has_state_reference = state_keywords
            .iter()
            .any(|keyword| perception_lower.contains(keyword.to_lowercase().as_str()));

        if !has_state_reference {
            return Some(CognitiveValidationResult::rejected(
                "感知阶段未引用具体的世界状态".to_string(),
                "请在感知阶段明确描述你的 HP、饥饿、口渴、体力等状态".to_string(),
            ));
        }

        None
    }

    /// 规则 4: 检测重复模式
    fn detect_repetition(&self, chain: &CognitiveChain) -> Option<CognitiveValidationResult> {
        // 检查相邻阶段的内容是否过于相似（可能的复制粘贴）
        let stages = &chain.stages;

        for i in 0..stages.len().saturating_sub(1) {
            let current = &stages[i].content;
            let next = &stages[i + 1].content;

            // 计算相似度（简化版：检查一个是否是另一个的子串）
            if current.len() > 10
                && next.len() > 10
                && (current.contains(next) || next.contains(current))
            {
                return Some(CognitiveValidationResult::rejected(
                    format!(
                        "{} 阶段和 {} 阶段内容过于相似，存在复制粘贴嫌疑",
                        stages[i].stage.name(),
                        stages[i + 1].stage.name()
                    ),
                    "请确保每个阶段有独立、独特的思考，避免复制前面的内容".to_string(),
                ));
            }
        }

        // 检查是否使用了通用的"偷懒"短语
        let lazy_patterns = ["好的", "知道了", "按计划", "直接", "一样", "没问题"];

        for stage_output in stages {
            for pattern in &lazy_patterns {
                if stage_output.content.trim() == *pattern {
                    return Some(CognitiveValidationResult::rejected(
                        format!("{} 阶段使用了过于简单的回复", stage_output.stage.name()),
                        format!("请提供更详细、具体的{}内容", stage_output.stage.name()),
                    ));
                }
            }
        }

        None
    }

    /// 规则 5: 检查推理连贯性
    fn check_coherence(&self, chain: &CognitiveChain) -> Option<CognitiveValidationResult> {
        // 检查决策阶段是否引用了前面的阶段
        let decision = chain.get_stage(CognitiveStage::Decision)?;

        // 检查是否引用了动机或规划
        let motivation = chain.get_stage(CognitiveStage::Motivation);
        let planning = chain.get_stage(CognitiveStage::Planning);

        let decision_lower = decision.content.to_lowercase();

        // 简单检查：决策是否提到了"因为"或"根据"
        let has_reasoning = decision_lower.contains("因为")
            || decision_lower.contains("由于")
            || decision_lower.contains("根据")
            || decision_lower.contains("基于");

        if !has_reasoning {
            // 检查是否在 content 中有对其他阶段的引用
            let references_previous = motivation
                .as_ref()
                .map(|m| {
                    decision_lower
                        .contains(&m.content.to_lowercase().chars().take(5).collect::<String>())
                        || decision_lower.contains(
                            &m.content
                                .to_lowercase()
                                .chars()
                                .take(10)
                                .collect::<String>(),
                        )
                })
                .unwrap_or(false)
                || planning
                    .as_ref()
                    .map(|p| {
                        decision_lower
                            .contains(&p.content.to_lowercase().chars().take(5).collect::<String>())
                            || decision_lower.contains(
                                &p.content
                                    .to_lowercase()
                                    .chars()
                                    .take(10)
                                    .collect::<String>(),
                            )
                    })
                    .unwrap_or(false);

            if !references_previous {
                return Some(CognitiveValidationResult::rejected(
                    "决策阶段未引用前面的思考结果".to_string(),
                    "请在决策中说明你的选择是如何基于感知、动机和规划的".to_string(),
                ));
            }
        }

        None
    }
}

// ============================================================================
// 带重试的认知引擎
// ============================================================================

/// 带重试机制的认知引擎包装器
///
/// 当验证失败时，自动重试最多指定次数
pub struct CognitiveEngineWithRetry {
    max_retries: usize,
    retry_delay_ms: u64,
}

impl CognitiveEngineWithRetry {
    /// 创建新的带重试的认知引擎
    pub fn new(max_retries: usize) -> Self {
        Self {
            max_retries,
            retry_delay_ms: 500,
        }
    }

    /// 设置重试延迟
    pub fn with_retry_delay(mut self, delay_ms: u64) -> Self {
        self.retry_delay_ms = delay_ms;
        self
    }

    /// 执行带重试的认知流程
    pub async fn think_with_retry<F, Fut>(
        &self,
        mut think_fn: F,
    ) -> Result<crate::core::cognitive::CognitiveChain>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<crate::core::cognitive::CognitiveChain>>,
    {
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            let chain = think_fn().await;

            match chain {
                Ok(c) => {
                    // 验证认知链
                    let validator = CognitiveValidator::new(c.persona.clone());
                    let result = validator.validate(&c);

                    if result.is_valid {
                        return Ok(c);
                    }

                    // 验证失败
                    if attempt < self.max_retries {
                        let reason = result.reason.clone().unwrap_or_default();
                        tracing::warn!(
                            "认知验证失败 (尝试 {}/{}): {}",
                            attempt + 1,
                            self.max_retries + 1,
                            reason
                        );

                        // 等待一段时间后重试
                        tokio::time::sleep(std::time::Duration::from_millis(self.retry_delay_ms))
                            .await;
                    } else {
                        let reason = result.reason.clone().unwrap_or_default();
                        tracing::error!("认知验证失败，已达最大重试次数: {}", reason);
                        return Err(anyhow::anyhow!("认知验证失败: {}", reason));
                    }
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < self.max_retries {
                        tokio::time::sleep(std::time::Duration::from_millis(self.retry_delay_ms))
                            .await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("认知流程失败")))
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

    fn create_test_chain() -> crate::core::cognitive::CognitiveChain {
        let mut chain = crate::core::cognitive::CognitiveChain::new(
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
        chain.final_intent = crate::models::Intent::new(Uuid::new_v4(), 1, "use", Some(serde_json::json!({"item_id": "mantou"})))
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
        let mut chain = crate::core::cognitive::CognitiveChain::new(
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

        let mut chain = crate::core::cognitive::CognitiveChain::new(
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

    #[test]
    fn test_min_thought_length_setting() {
        let validator = CognitiveValidator::new("测试人设".to_string()).with_min_thought_length(50);

        assert_eq!(validator.min_thought_length, 50);
    }

    #[test]
    fn test_strict_mode_setting() {
        let validator = CognitiveValidator::new("测试人设".to_string()).with_strict_mode(false);

        assert!(!validator.strict_mode);
    }

    #[test]
    fn test_validation_result_approved() {
        let result = CognitiveValidationResult::approved();
        assert!(result.is_valid);
        assert!(result.reason.is_none());
        assert!(result.suggestion.is_none());
    }

    #[test]
    fn test_validation_result_rejected() {
        let result =
            CognitiveValidationResult::rejected("测试原因".to_string(), "测试建议".to_string());

        assert!(!result.is_valid);
        assert_eq!(result.reason, Some("测试原因".to_string()));
        assert_eq!(result.suggestion, Some("测试建议".to_string()));
    }

    #[test]
    fn test_retry_wrapper_default() {
        let wrapper = CognitiveEngineWithRetry::new(3);
        assert_eq!(wrapper.max_retries, 3);
        assert_eq!(wrapper.retry_delay_ms, 500);
    }

    #[test]
    fn test_retry_wrapper_with_delay() {
        let wrapper = CognitiveEngineWithRetry::new(3).with_retry_delay(1000);

        assert_eq!(wrapper.max_retries, 3);
        assert_eq!(wrapper.retry_delay_ms, 1000);
    }
}
