// ============================================================================
// 多阶段认知引擎
// ============================================================================
//
// 实现 Perception → Motivation → Planning → Decision 的强制认知流程
//
// 核心设计理念：
// - 每个阶段独立调用 LLM，确保深度思考
// - 后阶段接收前阶段的输出，形成认知链
// - 最终决策必须基于完整的认知链
// ============================================================================

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::ai::cognitive::narrative::{NarrativeEngine, PerceptionNarrative};
use crate::ai::llm::{LlmClient, LlmClientExt};
use crate::models::{Intent, WorldState};
use crate::ai::persona::DynamicPersona;

// ============================================================================
// 认知阶段定义
// ============================================================================

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
    pub fn with_metadata(stage: CognitiveStage, content: String, metadata: serde_json::Value) -> Self {
        Self {
            stage,
            content,
            metadata,
        }
    }
}

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
            final_intent: Intent::idle(
                uuid::Uuid::new_v4(), // 临时 ID，后续会替换
                tick_id,
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
            final_intent: Intent::idle(
                uuid::Uuid::new_v4(), // 临时 ID，后续会替换
                tick_id,
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

        summary.push_str(&format!(
            "\n## 最终决策\n动作: {:?}\n思考: {}\n",
            self.final_intent.action_type,
            self.final_intent.thought_log.as_deref().unwrap_or("(无)")
        ));

        summary
    }
}

// ============================================================================
// 阶段响应类型定义
// ============================================================================

/// 感知阶段响应
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PerceptionResponse {
    /// 自身状态摘要
    self_status: String,
    /// 环境观察
    environment: String,
    /// 识别到的关键信息
    key_observations: Vec<String>,
}

/// 动机阶段响应
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MotivationResponse {
    /// 当前主要驱动力
    primary_drive: String,
    /// 驱动强度 (1-10)
    drive_intensity: u8,
    /// 为什么有这个动机
    reasoning: String,
}

/// 规划阶段响应
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanningResponse {
    /// 计划步骤
    steps: Vec<String>,
    /// 优先级 (1-10)
    priority: u8,
    /// 预期结果
    expected_outcome: String,
}

/// 决策阶段响应
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DecisionResponse {
    /// 思考过程（必须引用前面的阶段）
    thought_process: String,
    /// 选择的动作
    action: String,
    /// 目标（可选）
    target: Option<String>,
    /// 额外数据（可选）
    data: Option<String>,
}

// ============================================================================
// 多阶段认知引擎
// ============================================================================

/// 多阶段认知引擎配置
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
        use uuid::Uuid;

        let agent_id = Uuid::new_v4();
        let persona = DynamicPersona::new(
            agent_id,
            "无名侠客",
            "你是一名行走在江湖中的侠客。",
        );

        Self {
            agent_name: "无名侠客".to_string(),
            persona,
            temperature: 0.7,
            max_tokens_per_stage: 1024,
        }
    }
}

/// 多阶段认知引擎
///
/// 通过强制执行 Perception → Motivation → Planning → Decision 流程，
/// 确保 LLM 进行深度思考而非简单的条件反射。
pub struct MultiStageCognitiveEngine {
    llm_client: Arc<dyn LlmClient>,
    config: CognitiveEngineConfig,
}

impl MultiStageCognitiveEngine {
    /// 创建新的多阶段认知引擎
    pub fn new(llm_client: Arc<dyn LlmClient>, config: CognitiveEngineConfig) -> Self {
        Self { llm_client, config }
    }

    /// 使用默认配置创建
    pub fn with_defaults(llm_client: Arc<dyn LlmClient>) -> Self {
        Self::new(llm_client, CognitiveEngineConfig::default())
    }

    /// 执行完整认知流程
    pub async fn think(&self, world_state: &WorldState) -> Result<CognitiveChain> {
        let start_time = std::time::Instant::now();
        let tick_id = world_state.tick_id;

        info!("[{}-{}] 开始认知流程...", self.config.agent_name, tick_id);

        // 使用 DynamicPersona 生成认知链
        let mut chain = CognitiveChain::from_persona(&self.config.persona, tick_id);

        // === Stage 1: Perception (感知) ===
        debug!("执行 Stage 1: Perception");
        let perception = self.perceive(world_state).await?;
        chain.add_stage(perception);

        // === Stage 2: Motivation (动机) ===
        debug!("执行 Stage 2: Motivation");
        let perception_output = chain.get_stage(CognitiveStage::Perception).unwrap().clone();
        let motivation = self.motivate(world_state, &perception_output).await?;
        chain.add_stage(motivation);

        // === Stage 3: Planning (规划) ===
        debug!("执行 Stage 3: Planning");
        let motivation_output = chain.get_stage(CognitiveStage::Motivation).unwrap().clone();
        let planning = self.plan(world_state, &perception_output, &motivation_output).await?;
        chain.add_stage(planning);

        // === Stage 4: Decision (决策) ===
        debug!("执行 Stage 4: Decision");
        let (decision, intent) = self.decide(world_state, &chain).await?;
        chain.add_stage(decision);
        chain.final_intent = intent;

        // 记录耗时
        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        info!(
            "[{}-{}] 认知完成，耗时 {}ms",
            self.config.agent_name, tick_id, chain.duration_ms
        );

        Ok(chain)
    }

    /// 使用记忆上下文执行完整认知流程
    pub async fn think_with_memory(
        &self,
        world_state: &WorldState,
        memory_context: &str,
    ) -> Result<CognitiveChain> {
        let start_time = std::time::Instant::now();
        let tick_id = world_state.tick_id;

        // 使用 DynamicPersona 生成认知链
        let mut chain = CognitiveChain::from_persona(&self.config.persona, tick_id);

        // === Stage 1: Perception (感知) ===
        debug!("执行 Stage 1: Perception (with memory context)");
        let perception = self.perceive_with_memory(world_state, memory_context).await?;
        chain.add_stage(perception);

        // === Stage 2: Motivation (动机) ===
        debug!("执行 Stage 2: Motivation");
        let perception_output = chain.get_stage(CognitiveStage::Perception).unwrap().clone();
        let motivation = self.motivate(world_state, &perception_output).await?;
        chain.add_stage(motivation);

        // === Stage 3: Planning (规划) ===
        debug!("执行 Stage 3: Planning");
        let motivation_output = chain.get_stage(CognitiveStage::Motivation).unwrap().clone();
        let planning = self.plan(world_state, &perception_output, &motivation_output).await?;
        chain.add_stage(planning);

        // === Stage 4: Decision (决策) ===
        debug!("执行 Stage 4: Decision");
        let (decision, intent) = self.decide(world_state, &chain).await?;
        chain.add_stage(decision);
        chain.final_intent = intent;

        // 记录耗时
        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        info!(
            "[{}-{}] 认知完成，耗时 {}ms",
            self.config.agent_name, tick_id, chain.duration_ms
        );

        Ok(chain)
    }

    // ========================================================================
    // 各阶段实现
    // ========================================================================

    /// Stage 1: 感知 - 理解当前世界状态
    async fn perceive(&self, world_state: &WorldState) -> Result<StageOutput> {
        let prompt = self.build_perception_prompt(world_state);

        let response: PerceptionResponse = self.llm_client.complete_json(&prompt).await?;

        // 构建阶段输出
        let content = format!(
            "自身状态: {}\n环境: {}\n关键观察: {}",
            response.self_status,
            response.environment,
            response.key_observations.join(", ")
        );

        let metadata = serde_json::to_value(&response)?;
        Ok(StageOutput::with_metadata(CognitiveStage::Perception, content, metadata))
    }

    /// Stage 1 (带记忆): 感知 - 理解当前世界状态，融入记忆上下文
    async fn perceive_with_memory(&self, world_state: &WorldState, memory_context: &str) -> Result<StageOutput> {
        let prompt = self.build_perception_prompt_with_memory(world_state, memory_context);

        let response: PerceptionResponse = self.llm_client.complete_json(&prompt).await?;

        // 构建阶段输出
        let content = format!(
            "自身状态: {}\n环境: {}\n关键观察: {}",
            response.self_status,
            response.environment,
            response.key_observations.join(", ")
        );

        let metadata = serde_json::to_value(&response)?;
        Ok(StageOutput::with_metadata(CognitiveStage::Perception, content, metadata))
    }

    /// Stage 2: 动机 - 基于人设生成内在驱动力
    async fn motivate(&self, world_state: &WorldState, perception: &StageOutput) -> Result<StageOutput> {
        let prompt = self.build_motivation_prompt(world_state, perception);

        let response: MotivationResponse = self.llm_client.complete_json(&prompt).await?;

        let content = format!(
            "主要驱动力: {} (强度: {}/10)\n原因: {}",
            response.primary_drive, response.drive_intensity, response.reasoning
        );

        let metadata = serde_json::to_value(&response)?;
        Ok(StageOutput::with_metadata(CognitiveStage::Motivation, content, metadata))
    }

    /// Stage 3: 规划 - 制定行动计划
    async fn plan(
        &self,
        world_state: &WorldState,
        perception: &StageOutput,
        motivation: &StageOutput,
    ) -> Result<StageOutput> {
        let prompt = self.build_planning_prompt(world_state, perception, motivation);

        let response: PlanningResponse = self.llm_client.complete_json(&prompt).await?;

        let content = format!(
            "计划步骤:\n1. {}\n预期结果: {} (优先级: {}/10)",
            response.steps.join("\n2. "),
            response.expected_outcome,
            response.priority
        );

        let metadata = serde_json::to_value(&response)?;
        Ok(StageOutput::with_metadata(CognitiveStage::Planning, content, metadata))
    }

    /// Stage 4: 决策 - 选择最终行动
    ///
    /// 返回 (StageOutput, Intent) 元组，让调用者同时获得阶段输出和最终意图
    async fn decide(&self, world_state: &WorldState, chain: &CognitiveChain) -> Result<(StageOutput, Intent)> {
        let prompt = self.build_decision_prompt(world_state, chain);

        let response: DecisionResponse = self.llm_client.complete_json(&prompt).await?;

        // 转换为 Intent
        let agent_id = world_state.agent_id.unwrap_or_default();
        let tick_id = world_state.tick_id;

        let intent = self.parse_decision_response(agent_id, tick_id, &response)?;

        let content = format!(
            "思考: {}\n行动: {}",
            response.thought_process,
            format!("{:?}", intent.action_type)
        );

        let metadata = serde_json::to_value(&response)?;
        let stage_output = StageOutput::with_metadata(CognitiveStage::Decision, content, metadata);

        Ok((stage_output, intent))
    }

    // ========================================================================
    // Prompt 构建方法
    // ========================================================================

    fn build_perception_prompt(&self, world_state: &WorldState) -> String {
        self.build_perception_prompt_with_memory(world_state, "")
    }

    /// 构建带记忆上下文的感知 Prompt（数据驱动叙事化版本）
    fn build_perception_prompt_with_memory(&self, world_state: &WorldState, memory_context: &str) -> String {
        let self_state = &world_state.self_state;

        // 使用叙事引擎生成叙事化描述（数据驱动）
        let engine = NarrativeEngine::default();
        let narrative = PerceptionNarrative::from_attributes_with_engine(
            &engine,
            &self_state.attributes,
            &self_state.status_effects,
        );

        // 生成叙事化的状态描述
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

        // 记忆上下文（如果有）
        let memory_section = if memory_context.is_empty() {
            String::new()
        } else {
            format!(
                r#"
### 相关记忆
{memory_context}

"#
            )
        };

        format!(
            r#"# 感知阶段 (Perception)

你是 {agent_name}。
{persona}

## 当前游戏状态 (Tick {tick_id})

{self_status_section}
### 背包物品
{inventory}

### 位置
- 地点: {location}

### 环境
- 附近的人: {entities}
- 地上的物品: {items}
{memory_section}
## 任务
请分析你感知到的信息，输出 JSON 格式的感知结果。

**注意**：只需客观描述你看到的状态，不需要做任何决策。

## 输出格式
{{
  "self_status": "你的状态简述 (30字以内)",
  "environment": "环境描述 (30字以内)",
  "key_observations": ["观察1", "观察2", "..."]
}}
"#,
            agent_name = self.config.agent_name,
            persona = self.config.persona.generate_description(),
            tick_id = world_state.tick_id,
            self_status_section = self_status_section,
            inventory = inventory_str,
            location = world_state.location.name,
            entities = entities_str,
            items = items_str,
            memory_section = memory_section,
        )
    }

    fn build_motivation_prompt(&self, _world_state: &WorldState, perception: &StageOutput) -> String {
        format!(
            r#"# 动机阶段 (Motivation)

你是 {agent_name}。
{persona}

## 你感知到的
{perception_content}

## 任务
基于你的感知和性格，说明你的内在驱动力。
- 你现在最想要什么？
- 为什么想要这个？
- 这个动机有多强烈？

## 输出格式
{{
  "primary_drive": "你当前的主要驱动力 (如'获取食物'、'避免危险'、'赚取银两')",
  "drive_intensity": 1-10,
  "reasoning": "为什么有这个动机 (50字以内)"
}}
"#,
            agent_name = self.config.agent_name,
            persona = self.config.persona.generate_description(),
            perception_content = perception.content
        )
    }

    fn build_planning_prompt(
        &self,
        _world_state: &WorldState,
        perception: &StageOutput,
        motivation: &StageOutput,
    ) -> String {
        format!(
            r#"# 规划阶段 (Planning)

你是 {agent_name}。
{persona}

## 感知
{perception}

## 动机
{motivation}

## 任务
基于你的感知和动机，制定行动计划。
- 你打算怎么做？
- 分成几个步骤？
- 预期结果是什么？

## 输出格式
{{
  "steps": ["步骤1", "步骤2", "..."],
  "priority": 1-10,
  "expected_outcome": "预期结果 (30字以内)"
}}
"#,
            agent_name = self.config.agent_name,
            persona = self.config.persona.generate_description(),
            perception = perception.content,
            motivation = motivation.content
        )
    }

    fn build_decision_prompt(&self, _world_state: &WorldState, chain: &CognitiveChain) -> String {
        // 获取各阶段输出作为上下文
        let perception = chain.get_stage(CognitiveStage::Perception)
            .map(|s| s.content.as_str())
            .unwrap_or("");
        let motivation = chain.get_stage(CognitiveStage::Motivation)
            .map(|s| s.content.as_str())
            .unwrap_or("");
        let planning = chain.get_stage(CognitiveStage::Planning)
            .map(|s| s.content.as_str())
            .unwrap_or("");

        format!(
            r#"# 决策阶段 (Decision)

你是 {agent_name}。
{persona}

## 前面的思考

### 感知
{perception}

### 动机
{motivation}

### 规划
{planning}

## 任务
基于你前面的思考，做出最终决策。
- 你要执行什么动作？
- 为什么选择这个动作？
- 动作的目标是谁？（如果适用）

## 重要约束
1. **必须引用前面的思考**：在 thought_process 中说明你的决策如何基于感知、动机和规划。
2. **不能跳过思考**：不能直接说"因为饿了所以吃"，必须体现完整的认知链条。
3. **必须以 JSON 格式输出**：不要包含其他文本。

## 输出格式
{{
  "thought_process": "你的完整思考过程，必须引用感知、动机和规划 (300字以内)",
  "action": "动作名称 (idle, speak, move, use, attack, pickup, give, trade, steal)",
  "target": "目标名称或ID (可选)",
  "data": "额外数据，如说话内容 (可选)"
}}
"#,
            agent_name = self.config.agent_name,
            persona = self.config.persona.generate_description()
        )
    }

    // ========================================================================
    // 辅助方法
    // ========================================================================

    /// 解析决策响应为 Intent
    fn parse_decision_response(&self, agent_id: uuid::Uuid, tick_id: i64, response: &DecisionResponse) -> Result<Intent> {
        let action = response.action.to_lowercase();
        let target = response.target.as_deref();
        let data = response.data.as_deref();

        let mut intent = match action.as_str() {
            "idle" => Intent::idle(agent_id, tick_id),
            "speak" => {
                let content = data.unwrap_or("...").to_string();
                Intent::speak(agent_id, tick_id, content)
            }
            "move" => {
                // MVP 阶段暂时不支持移动
                Intent::idle(agent_id, tick_id)
                    .with_thought("想要移动，但暂不支持".to_string())
            }
            "use" => {
                if let Some(_item_name) = target {
                    Intent::use_item(agent_id, tick_id, _item_name)
                } else {
                    Intent::idle(agent_id, tick_id)
                        .with_thought("想用东西但不知道用什么".to_string())
                }
            }
            "attack" => {
                if let Some(_target_name) = target {
                    Intent::attack(agent_id, tick_id, uuid::Uuid::new_v4()) // 需要解析名字为 ID
                } else {
                    Intent::idle(agent_id, tick_id)
                        .with_thought("想攻击但没有目标".to_string())
                }
            }
            "pickup" => {
                if let Some(_item_name) = target {
                    Intent::pickup(agent_id, tick_id, _item_name)
                } else {
                    Intent::idle(agent_id, tick_id)
                        .with_thought("想捡东西但不知道捡什么".to_string())
                }
            }
            "give" => {
                if let Some(_target_name) = target {
                    Intent::give(agent_id, tick_id, uuid::Uuid::new_v4(), data.unwrap_or_default(), 1)
                } else {
                    Intent::idle(agent_id, tick_id)
                        .with_thought("想给东西但没有目标".to_string())
                }
            }
            "trade" => {
                Intent::idle(agent_id, tick_id)
                    .with_thought("交易功能开发中".to_string())
            }
            "steal" => {
                if let Some(_target_name) = target {
                    Intent::steal(agent_id, tick_id, uuid::Uuid::new_v4(), data.unwrap_or_default())
                } else {
                    Intent::idle(agent_id, tick_id)
                        .with_thought("想偷东西但没有目标".to_string())
                }
            }
            _ => {
                warn!("未知动作: {}", action);
                Intent::idle(agent_id, tick_id)
                    .with_thought(format!("未知动作: {}", action))
            }
        };

        // 附加思考过程
        intent.thought_log = Some(response.thought_process.clone());
        Ok(intent)
    }
}

// ============================================================================
// 创建 DecisionCallback 的便捷方法
// ============================================================================

impl MultiStageCognitiveEngine {
    /// 创建决策回调（兼容现有 Agent 接口）
    pub fn create_decision_callback(self) -> crate::runtime::decision::DecisionCallback {
        let engine = Arc::new(self);
        Arc::new(move |world_state: &WorldState| {
            let engine = engine.clone();
            let world_state = world_state.clone();
            Box::pin(async move {
                match engine.think(&world_state).await {
                    Ok(chain) => chain.final_intent,
                    Err(e) => {
                        tracing::error!("多阶段认知失败: {}", e);
                        Intent::idle(
                            world_state.agent_id.unwrap_or_default(),
                            world_state.tick_id,
                        )
                        .with_thought(format!("认知受阻: {}", e))
                    }
                }
            })
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::mock::MockLlmClient;

    #[tokio::test]
    async fn test_cognitive_stages() {
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
    fn test_cognitive_chain() {
        let mut chain = CognitiveChain::new("测试侠客".to_string(), "测试人设".to_string(), 1);

        assert!(!chain.is_complete());

        chain.add_stage(StageOutput::new(CognitiveStage::Perception, "感知内容".to_string()));
        chain.add_stage(StageOutput::new(CognitiveStage::Motivation, "动机内容".to_string()));
        chain.add_stage(StageOutput::new(CognitiveStage::Planning, "规划内容".to_string()));
        chain.add_stage(StageOutput::new(CognitiveStage::Decision, "决策内容".to_string()));

        assert!(chain.is_complete());
    }

    #[test]
    fn test_cognitive_engine_config_default() {
        let config = CognitiveEngineConfig::default();
        assert_eq!(config.agent_name, "无名侠客");
        assert_eq!(config.temperature, 0.7);
    }
}
