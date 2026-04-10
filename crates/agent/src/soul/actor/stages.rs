// ============================================================================
// 认知阶段定义
// ============================================================================

use serde::{Deserialize, Serialize};

/// 认知阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CognitiveStage {
    /// 感知：理解当前世界状态
    Perception,
    /// 动机：基于人设生成内在驱动力
    Motivation,
    /// 规划：制定行动计划
    Planning,
    /// 决策：选择最终行动
    Decision,
}

impl CognitiveStage {
    /// 获取阶段名称
    pub fn name(&self) -> &str {
        match self {
            Self::Perception => "感知",
            Self::Motivation => "动机",
            Self::Planning => "规划",
            Self::Decision => "决策",
        }
    }

    /// 获取所有阶段的顺序列表
    pub fn all() -> Vec<Self> {
        vec![
            Self::Perception,
            Self::Motivation,
            Self::Planning,
            Self::Decision,
        ]
    }
}

/// 阶段输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageOutput {
    /// 阶段类型
    pub stage: CognitiveStage,
    /// 阶段内容（LLM 输出的原始文本）
    pub content: String,
    /// 结构化元数据（解析后的关键信息）
    pub metadata: serde_json::Value,
}

impl StageOutput {
    /// 创建新的阶段输出
    pub fn new(stage: CognitiveStage, content: String) -> Self {
        Self {
            stage,
            content,
            metadata: serde_json::json!({}),
        }
    }

    /// 创建带元数据的阶段输出
    pub fn with_metadata(
        stage: CognitiveStage,
        content: String,
        metadata: serde_json::Value,
    ) -> Self {
        Self {
            stage,
            content,
            metadata,
        }
    }
}

/// 感知+动机合并阶段响应
///
/// Perception 和 Motivation 合并为单次 LLM 调用，
/// 同时输出观察结果和内在驱动力。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionMotivationResponse {
    /// 自身状态摘要
    pub self_status: String,
    /// 环境观察
    pub environment: String,
    /// 识别到的关键信息
    pub key_observations: Vec<String>,
    /// 当前主要驱动力
    pub primary_drive: String,
    /// 驱动强度 (1-10)
    pub drive_intensity: u8,
    /// 为什么有这个动机
    pub reasoning: String,
}

/// 规划+决策合并阶段响应
///
/// Planning 和 Decision 合并为单次 LLM 调用，
/// 同时输出行动计划和最终决策。
///
/// ActorSoul（人魂）只输出叙事意图，不输出结构化 action_data。
/// 结构化翻译由天魂（IntentTranslator）负责。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDecisionResponse {
    /// 计划步骤
    pub steps: Vec<String>,
    /// 优先级 (1-10)
    pub priority: u8,
    /// 预期结果
    pub expected_outcome: String,
    /// 思考过程（必须引用前面的感知和动机）
    pub thought_process: String,
    /// 叙事意图（自然语言描述想要做的事，如"吃一个馒头来充饥"）
    pub narrative_action: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_output() {
        let output = StageOutput::new(CognitiveStage::Perception, "test".to_string());
        assert_eq!(output.stage, CognitiveStage::Perception);
        assert_eq!(output.content, "test");

        let metadata = serde_json::json!({"key": "value"});
        let output_with_meta =
            StageOutput::with_metadata(CognitiveStage::Perception, "test".to_string(), metadata);
        assert_eq!(output_with_meta.metadata["key"], "value");
    }
}
