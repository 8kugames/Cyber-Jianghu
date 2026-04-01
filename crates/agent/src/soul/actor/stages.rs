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

/// 感知阶段响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionResponse {
    /// 自身状态摘要
    pub self_status: String,
    /// 环境观察
    pub environment: String,
    /// 识别到的关键信息
    pub key_observations: Vec<String>,
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

/// 动机阶段响应（保留用于兼容）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotivationResponse {
    /// 当前主要驱动力
    pub primary_drive: String,
    /// 驱动强度 (1-10)
    pub drive_intensity: u8,
    /// 为什么有这个动机
    pub reasoning: String,
}

/// 规划阶段响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanningResponse {
    /// 计划步骤
    pub steps: Vec<String>,
    /// 优先级 (1-10)
    pub priority: u8,
    /// 预期结果
    pub expected_outcome: String,
}

/// 决策阶段响应
///
/// 数据驱动：action 为动作名，action_data 直接透传到服务端。
/// LLM 必须按服务端要求的字段名输出 action_data，无需 agent 端硬编码映射。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionResponse {
    /// 思考过程（必须引用前面的阶段）
    pub thought_process: String,
    /// 选择的动作（对应 actions.yaml 中的 key）
    pub action: String,
    /// 动作参数（直接透传到服务端，字段名必须与 actions.yaml 中 required_fields 一致）
    #[serde(default)]
    pub action_data: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cognitive_stages() {
        let stages = CognitiveStage::all();
        assert_eq!(stages.len(), 4);
        assert_eq!(stages[0], CognitiveStage::Perception);
        assert_eq!(stages[3], CognitiveStage::Decision);
    }

    #[test]
    fn test_stage_names() {
        assert_eq!(CognitiveStage::Perception.name(), "感知");
        assert_eq!(CognitiveStage::Motivation.name(), "动机");
        assert_eq!(CognitiveStage::Planning.name(), "规划");
        assert_eq!(CognitiveStage::Decision.name(), "决策");
    }

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
