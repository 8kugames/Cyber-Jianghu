// ============================================================================
// 认知链管理
// ============================================================================

use serde::{Deserialize, Serialize};

use super::stages::{CognitiveStage, StageOutput};
use crate::component::persona::DynamicPersona;
use crate::models::Intent;

/// 完整认知链
///
/// 记录从 Perception 到 Decision 的完整思考过程
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveChain {
    /// Agent 名称
    pub agent_name: String,
    /// Agent 人设
    pub persona: String,
    /// 当前 Tick ID
    pub tick_id: i64,
    /// 各阶段输出
    pub stages: Vec<StageOutput>,
    /// 最终意图
    pub final_intent: Intent,
    /// 认知耗时（毫秒）
    pub duration_ms: u64,
}

impl CognitiveChain {
    /// 创建新的认知链
    pub fn new(agent_name: String, persona_description: String, tick_id: i64) -> Self {
        Self {
            agent_name,
            persona: persona_description,
            tick_id,
            stages: Vec::new(),
            final_intent: Intent::new(
                uuid::Uuid::new_v4(), // 临时 ID，后续会替换
                tick_id,
                "idle",
                None,
            ),
            duration_ms: 0,
        }
    }

    /// 从 DynamicPersona 创建认知链
    pub fn from_persona(persona: &DynamicPersona, tick_id: i64) -> Self {
        Self {
            agent_name: persona.name.clone(),
            persona: persona.generate_description(),
            tick_id,
            stages: Vec::new(),
            final_intent: Intent::new(
                uuid::Uuid::new_v4(), // 临时 ID，后续会替换
                tick_id,
                "idle",
                None,
            ),
            duration_ms: 0,
        }
    }

    /// 添加阶段输出
    pub fn add_stage(&mut self, output: StageOutput) {
        self.stages.push(output);
    }

    /// 获取指定阶段的输出
    pub fn get_stage(&self, stage: CognitiveStage) -> Option<&StageOutput> {
        self.stages.iter().find(|s| s.stage == stage)
    }

    /// 检查认知链是否完整
    pub fn is_complete(&self) -> bool {
        self.stages.len() == CognitiveStage::all().len()
    }

    /// 生成人类可读的认知摘要
    pub fn summarize(&self) -> String {
        let mut summary = format!("【{} 认知链 - Tick {}】\n", self.agent_name, self.tick_id);

        for stage_output in &self.stages {
            summary.push_str(&format!(
                "\n## {} 阶段\n{}\n",
                stage_output.stage.name(),
                stage_output.content
            ));
        }

        let narrative = self.final_intent.action_data
            .as_ref()
            .and_then(|d| d.get("narrative"))
            .and_then(|n| n.as_str())
            .unwrap_or("(未生成叙事意图)");

        summary.push_str(&format!(
            "\n## 叙事意图\n{}\n思考: {}\n",
            narrative,
            self.final_intent.thought_log.as_deref().unwrap_or("(无)")
        ));

        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cognitive_chain() {
        let mut chain = CognitiveChain::new("测试侠客".to_string(), "测试人设".to_string(), 1);

        assert!(!chain.is_complete());

        chain.add_stage(StageOutput::new(
            CognitiveStage::Perception,
            "感知内容".to_string(),
        ));
        chain.add_stage(StageOutput::new(
            CognitiveStage::Motivation,
            "动机内容".to_string(),
        ));
        chain.add_stage(StageOutput::new(
            CognitiveStage::Planning,
            "规划内容".to_string(),
        ));
        chain.add_stage(StageOutput::new(
            CognitiveStage::Decision,
            "决策内容".to_string(),
        ));

        assert!(chain.is_complete());
    }

    #[test]
    fn test_get_stage() {
        let mut chain = CognitiveChain::new("测试".to_string(), "测试人设".to_string(), 1);
        chain.add_stage(StageOutput::new(
            CognitiveStage::Perception,
            "感知".to_string(),
        ));

        let perception = chain.get_stage(CognitiveStage::Perception);
        assert!(perception.is_some());
        assert_eq!(perception.unwrap().content, "感知");

        let motivation = chain.get_stage(CognitiveStage::Motivation);
        assert!(motivation.is_none());
    }

    #[test]
    fn test_summarize() {
        let mut chain = CognitiveChain::new("测试侠客".to_string(), "测试人设".to_string(), 1);
        chain.add_stage(StageOutput::new(
            CognitiveStage::Perception,
            "感知内容".to_string(),
        ));
        chain.final_intent = Intent::new(uuid::Uuid::new_v4(), 1, "idle", None);

        let summary = chain.summarize();
        assert!(summary.contains("测试侠客"));
        assert!(summary.contains("感知内容"));
        assert!(summary.contains("叙事意图"));
    }
}
