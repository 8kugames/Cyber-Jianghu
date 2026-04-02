// ============================================================================
// 认知引擎核心（5 阶段，非线性管道）
// ============================================================================
//
// 5 个认知阶段通过 2 次合并 LLM 调用执行，降低 token 消耗和延迟：
//   1. 感知 + 2. 动机 → LLM Call 1（Perception+Motivation 合并）
//   3. 规划 + 4. 决策 → LLM Call 2（Planning+Decision 合并）
//   5. 验证           → ReflectorSoul（engine 外部执行）
// ============================================================================

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use super::chain::CognitiveChain;
use super::stages::{
    CognitiveStage, PerceptionMotivationResponse, PlanDecisionResponse, StageOutput,
};
use crate::component::llm::{LlmClient, LlmClientExt};
use crate::component::persona::DynamicPersona;
use crate::infra::api::thinking_log;
use crate::models::{Intent, WorldState};
use crate::soul::actor::narrative::{NarrativeEngine, PerceptionNarrative};

/// 认知引擎配置
#[derive(Clone, Debug)]
pub struct CognitiveEngineConfig {
    /// Agent 名称
    pub agent_name: String,
    /// Agent 动态人设
    pub persona: DynamicPersona,
    /// 温度参数
    pub temperature: f32,
    /// 每阶段最大 token 数
    pub max_tokens_per_stage: u32,
}

impl Default for CognitiveEngineConfig {
    fn default() -> Self {
        let agent_id = uuid::Uuid::new_v4();
        let persona = DynamicPersona::new(agent_id, "无名侠客", "你是一名行走在江湖中的侠客。");

        Self {
            agent_name: "无名侠客".to_string(),
            persona,
            temperature: 0.7,
            max_tokens_per_stage: 1024,
        }
    }
}

/// 认知引擎（5 阶段，非线性管道）
///
/// 5 个认知阶段通过 2 次合并 LLM 调用执行：
/// - LLM Call 1: Perception + Motivation（感知+动机）
/// - LLM Call 2: Planning + Decision（规划+决策）
/// - Validation 由 ReflectorSoul 在 engine 外部执行
pub struct CognitiveEngine {
    llm_client: Arc<dyn LlmClient>,
    config: CognitiveEngineConfig,
}

impl CognitiveEngine {
    /// 创建新的认知引擎
    pub fn new(llm_client: Arc<dyn LlmClient>, config: CognitiveEngineConfig) -> Self {
        Self { llm_client, config }
    }

    /// 使用默认配置创建
    pub fn with_defaults(llm_client: Arc<dyn LlmClient>) -> Self {
        Self::new(llm_client, CognitiveEngineConfig::default())
    }

    pub async fn think(&self, world_state: &WorldState) -> Result<CognitiveChain> {
        self.think_with_feedback(world_state, None).await
    }

    pub async fn think_with_feedback(
        &self,
        world_state: &WorldState,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        self.think_with_memory_and_feedback(world_state, "", validation_feedback)
            .await
    }

    /// 使用记忆上下文执行认知流程
    pub async fn think_with_memory(
        &self,
        world_state: &WorldState,
        memory_context: &str,
    ) -> Result<CognitiveChain> {
        self.think_with_memory_and_feedback(world_state, memory_context, None)
            .await
    }

    /// 核心认知流程：支持 memory context + validation feedback
    pub(crate) async fn think_with_memory_and_feedback(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        let start_time = std::time::Instant::now();
        let tick_id = world_state.tick_id;

        info!("[{}-{}] 开始认知流程...", self.config.agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&self.config.persona, tick_id);

        // 缓存 persona description（同一 tick 内人设不变）
        let persona_desc = self.config.persona.generate_description();

        // === Stage 1: Perception+Motivation (感知+动机，合并为单次 LLM 调用) ===
        let prompt = self.build_perception_motivation_prompt(
            world_state,
            memory_context,
            validation_feedback,
            &persona_desc,
        );
        let (pm_response, perception, motivation) = self.perceive_and_motivate(&prompt).await?;
        chain.add_stage(perception);
        chain.add_stage(motivation);
        thinking_log::log_llm(
            &self.config.agent_name,
            tick_id,
            "Perception+Motivation",
            &prompt,
            &pm_response,
        );

        // === Stage 2: Plan+Decide (规划+决策，合并为单次 LLM 调用) ===
        debug!("执行 Stage 2: Plan+Decide");
        let perception_output = chain.get_stage(CognitiveStage::Perception).unwrap().clone();
        let motivation_output = chain.get_stage(CognitiveStage::Motivation).unwrap().clone();
        let pd_prompt =
            self.build_plan_decision_prompt(&perception_output, &motivation_output, &persona_desc);
        let (pd_response, planning, decision, intent) =
            self.plan_and_decide(&pd_prompt, world_state).await?;
        chain.add_stage(planning);
        chain.add_stage(decision);
        chain.final_intent = intent;
        thinking_log::log_llm(
            &self.config.agent_name,
            tick_id,
            "Planning+Decision",
            &pd_prompt,
            &pd_response,
        );

        // 记录耗时
        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        info!(
            "[{}-{}] 认知完成，耗时 {}ms",
            self.config.agent_name, tick_id, chain.duration_ms
        );

        thinking_log::log_thinking(&self.config.agent_name, tick_id, &chain.summarize());

        Ok(chain)
    }

    // ========================================================================
    // 各阶段实现（接收预构建的 prompt，避免重复构建）
    // ========================================================================

    /// Stage 1: 感知+动机（合并为单次 LLM 调用）
    async fn perceive_and_motivate(
        &self,
        prompt: &str,
    ) -> Result<(String, StageOutput, StageOutput)> {
        let response: PerceptionMotivationResponse = self.llm_client.complete_json(prompt).await?;

        let response_json = serde_json::to_string(&response)?;
        let _metadata = serde_json::to_value(&response)?;

        let perception_content = format!(
            "自身状态: {}\n环境: {}\n关键观察: {}",
            response.self_status,
            response.environment,
            response.key_observations.join(", ")
        );
        let perception = StageOutput::with_metadata(
            CognitiveStage::Perception,
            perception_content,
            serde_json::json!({
                "self_status": response.self_status,
                "environment": response.environment,
                "key_observations": response.key_observations,
            }),
        );

        let motivation_content = format!(
            "主要驱动力: {} (强度: {}/10)\n原因: {}",
            response.primary_drive, response.drive_intensity, response.reasoning
        );
        let motivation = StageOutput::with_metadata(
            CognitiveStage::Motivation,
            motivation_content,
            serde_json::json!({
                "primary_drive": response.primary_drive,
                "drive_intensity": response.drive_intensity,
                "reasoning": response.reasoning,
            }),
        );

        Ok((response_json, perception, motivation))
    }

    /// Stage 2: 规划+决策（合并为单次 LLM 调用）
    async fn plan_and_decide(
        &self,
        prompt: &str,
        world_state: &WorldState,
    ) -> Result<(String, StageOutput, StageOutput, Intent)> {
        let mut response: PlanDecisionResponse = self.llm_client.complete_json(prompt).await?;

        let response_json = serde_json::to_string(&response)?;

        // Planning stage output
        let planning_content = format!(
            "计划步骤:\n1. {}\n预期结果: {} (优先级: {}/10)",
            response.steps.join("\n2. "),
            response.expected_outcome,
            response.priority
        );
        let planning = StageOutput::with_metadata(
            CognitiveStage::Planning,
            planning_content,
            serde_json::json!({
                "steps": response.steps,
                "priority": response.priority,
                "expected_outcome": response.expected_outcome,
            }),
        );

        // Decision stage output
        let agent_id = world_state.agent_id.unwrap_or_default();
        let tick_id = world_state.tick_id;
        let action_type = response.action.to_lowercase();

        let action_data = if response.action_data.is_null() {
            None
        } else {
            Some(std::mem::take(&mut response.action_data))
        };

        let intent = Intent::new(agent_id, tick_id, action_type.as_str(), action_data)
            .with_thought(response.thought_process.clone());

        let decision_content = format!(
            "思考: {}\n行动: {}",
            response.thought_process, intent.action_type
        );
        let decision = StageOutput::with_metadata(
            CognitiveStage::Decision,
            decision_content,
            serde_json::to_value(&response)?,
        );

        Ok((response_json, planning, decision, intent))
    }

    // ========================================================================
    // Prompt 构建方法
    // ========================================================================

    fn build_perception_motivation_prompt(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
        persona_desc: &str,
    ) -> String {
        let self_state = &world_state.self_state;

        let engine = NarrativeEngine::default();
        let narrative = PerceptionNarrative::from_attributes_with_engine(
            &engine,
            &self_state.attributes,
            &self_state.status_effects,
        );
        let self_status_section = narrative.to_prompt_section();

        let inventory_str = if self_state.inventory.is_empty() {
            "空".to_string()
        } else {
            self_state
                .inventory
                .iter()
                .map(|i| format!("{} x{}", i.name, i.quantity))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let entities_str = if world_state.entities.is_empty() {
            "无".to_string()
        } else {
            world_state
                .entities
                .iter()
                .map(|e| format!("{}({})", e.name, e.state))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let items_str = if world_state.nearby_items.is_empty() {
            "无".to_string()
        } else {
            world_state
                .nearby_items
                .iter()
                .map(|i| i.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        };

        let memory_section = if memory_context.is_empty() {
            String::new()
        } else {
            format!("\n### 相关记忆\n{memory_context}\n")
        };

        let feedback_section = match validation_feedback {
            Some(fb) => format!("\n[验证反馈]: {}\n", fb),
            None => String::new(),
        };

        let adjacent_locations = if world_state.location.adjacent_nodes.is_empty() {
            "无（当前位置无法移动）".to_string()
        } else {
            world_state
                .location
                .adjacent_nodes
                .iter()
                .map(|n| {
                    if n.travel_cost > 1 {
                        format!("{} [{}] (耗时{}tick)", n.name, n.node_id, n.travel_cost)
                    } else {
                        format!("{} [{}]", n.name, n.node_id)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };

        format!(
            r#"# 感知与动机阶段 (Perception + Motivation)
{feedback_section}
你是 {agent_name}。
{persona}

## 当前游戏状态 (Tick {tick_id})

{self_status_section}
### 背包物品
{inventory}

### 位置
- 地点: {location}
- 可达位置: {adjacent_locations}

### 环境
- 附近的人: {entities}
- 地上的物品: {items}
{memory_section}
## 任务
分析你感知到的世界状态，并基于你的性格说明内在驱动力。

## 输出格式
{{
  "self_status": "你的状态简述 (30字以内)",
  "environment": "环境描述 (30字以内)",
  "key_observations": ["观察1", "观察2", "..."],
  "primary_drive": "你当前的主要驱动力 (如'获取食物'、'避免危险'、'赚取银两')",
  "drive_intensity": 5,
  "reasoning": "为什么有这个动机 (50字以内)"
}}
"#,
            agent_name = self.config.agent_name,
            persona = persona_desc,
            tick_id = world_state.tick_id,
            self_status_section = self_status_section,
            inventory = inventory_str,
            location = world_state.location.name,
            adjacent_locations = adjacent_locations,
            entities = entities_str,
            items = items_str,
            memory_section = memory_section,
            feedback_section = feedback_section,
        )
    }

    fn build_plan_decision_prompt(
        &self,
        perception: &StageOutput,
        motivation: &StageOutput,
        persona_desc: &str,
    ) -> String {
        format!(
            r#"# 规划与决策阶段 (Planning + Decision)

你是 {agent_name}。
{persona}

## 感知
{perception}

## 动机
{motivation}

## 任务
基于你的感知和动机，制定行动计划并做出最终决策。
1. 先规划：你打算怎么做？分成几个步骤？
2. 再决策：基于规划，选择一个具体的行动。

## 重要约束
1. **必须引用前面的思考**：在 thought_process 中说明你的决策如何基于感知和动机。
2. **不能跳过思考**：必须体现完整的认知链条。
3. **必须以 JSON 格式输出**：不要包含其他文本。

## 输出格式
{{
  "steps": ["步骤1", "步骤2", "..."],
  "priority": 5,
  "expected_outcome": "预期结果 (30字以内)",
  "thought_process": "你的完整思考过程，必须引用感知和动机 (300字以内)",
  "action": "动作名称",
  "action_data": {{}}
}}

## 可用动作及 action_data 字段（字段名必须严格匹配，否则服务端会拒绝）

| action | action_data 必填字段 | 说明 |
|--------|---------------------|------|
| idle | (无) | 休息 |
| speak | {{"content": "说的话"}} | 公开说话，所有人可见 |
| move | {{"target_location": "node_id（必须使用方括号内的node_id，如 longmen_backyard，不能使用中文名称）"}} | 移动到指定位置 |
| use | {{"item_id": "物品名"}} | 使用背包中的物品 |
| attack | {{"target_agent_id": "目标AgentID"}} | 攻击目标 |
| pickup | {{"item_id": "物品名"}} | 从地面拾取物品 |
| give | {{"target_agent_id": "目标AgentID", "item_id": "物品名", "quantity": 数量}} | 给予物品 |
| steal | {{"target_agent_id": "目标AgentID", "item_id": "物品名"}} | 偷取物品 |
| trade | {{"target_agent_id": "目标AgentID", "item_id": "物品名", "price": 价格}} | 交易 |
| drop | {{"item_id": "物品名", "quantity": 数量}} | 丢弃物品 |
| gather | {{"target_id": "采集目标ID"}} | 采集资源 |
| craft | {{"recipe_id": "配方ID"}} | 制造物品 |

注意：target_agent_id 从 entities 列表中获取，item_id 从 inventory 或 nearby_items 中获取。
"#,
            agent_name = self.config.agent_name,
            persona = persona_desc,
            perception = perception.content,
            motivation = motivation.content
        )
    }

    // ========================================================================
    // 辅助方法
    // ========================================================================
}

// ============================================================================
// 创建 DecisionCallback 的便捷方法
// ============================================================================

impl CognitiveEngine {
    /// 创建决策回调（兼容现有 Agent 接口）
    pub fn create_decision_callback(self) -> crate::runtime::DecisionCallback {
        let engine = Arc::new(self);
        Arc::new(move |world_state: &WorldState| {
            let engine = engine.clone();
            let world_state = world_state.clone();
            Box::pin(async move {
                match engine.think(&world_state).await {
                    Ok(chain) => chain.final_intent,
                    Err(e) => {
                        tracing::error!("多阶段认知失败: {}", e);
                        Intent::new(
                            world_state.agent_id.unwrap_or_default(),
                            world_state.tick_id,
                            "idle",
                            None,
                        )
                        .with_thought(format!("认知受阻: {}", e))
                    }
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cognitive_engine_config_default() {
        let config = CognitiveEngineConfig::default();
        assert_eq!(config.agent_name, "无名侠客");
        assert_eq!(config.temperature, 0.7);
    }
}
