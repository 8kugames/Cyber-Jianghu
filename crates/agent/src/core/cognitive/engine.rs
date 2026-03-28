// ============================================================================
// 多阶段认知引擎核心
// ============================================================================
//
// 实现 Perception → MotivationPlanning → Decision 的认知流程
//
// 优化历史：
// - v1: Perception → Motivation → Planning → Decision (4 次 LLM，~181s)
// - v2: Perception → MotivationPlanning → Decision (3 次 LLM，~135s)
//
// 核心设计理念：
// - 每个阶段独立调用 LLM，确保深度思考
// - 动机+规划合并为一次调用，减少延迟
// - 每阶段有超时保护，超时时回退到默认响应
// ============================================================================

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use super::chain::CognitiveChain;
use super::stages::{
    CognitiveStage, DecisionResponse, MotivationPlanningResponse, MotivationResponse,
    PerceptionResponse, PlanningResponse, StageOutput,
};
use crate::ai::cognitive::narrative::{NarrativeEngine, PerceptionNarrative};
use crate::ai::llm::{LlmClient, LlmClientExt};
use crate::ai::persona::DynamicPersona;
use crate::models::{Intent, WorldState};
use crate::runtime::decision::http::thinking_log;

/// 每阶段 LLM 调用超时（秒）
const STAGE_TIMEOUT_SECS: u64 = 45;

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

/// 多阶段认知引擎
///
/// 通过强制执行 Perception → MotivationPlanning → Decision 流程，
/// 确保 LLM 进行深度思考而非简单的条件反射。
///
/// Motivation + Planning 合并为一次 LLM 调用，减少延迟。
pub struct MultiStageCognitiveEngine {
    llm_client: Arc<dyn LlmClient>,
    config: std::sync::RwLock<CognitiveEngineConfig>,
}

impl MultiStageCognitiveEngine {
    /// 创建新的多阶段认知引擎
    pub fn new(llm_client: Arc<dyn LlmClient>, config: CognitiveEngineConfig) -> Self {
        Self {
            llm_client,
            config: std::sync::RwLock::new(config),
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults(llm_client: Arc<dyn LlmClient>) -> Self {
        Self::new(llm_client, CognitiveEngineConfig::default())
    }

    /// 获取配置的快照（用于 prompt 构建等只读场景）
    fn config_snapshot(&self) -> CognitiveEngineConfig {
        self.config.read().unwrap().clone()
    }

    /// 更新人设（角色注册/切换后调用）
    pub fn update_persona(&self, name: &str, system_prompt: &str) {
        let mut cfg = self.config.write().unwrap();
        cfg.agent_name = name.to_string();
        cfg.persona = DynamicPersona::new(
            uuid::Uuid::new_v4(),
            name,
            system_prompt,
        );
        info!("认知引擎人设已更新: {}", name);
    }

    pub async fn think(&self, world_state: &WorldState) -> Result<CognitiveChain> {
        self.think_with_feedback(world_state, None).await
    }

    pub async fn think_with_feedback(
        &self,
        world_state: &WorldState,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        let cfg = self.config_snapshot();
        let start_time = std::time::Instant::now();
        let tick_id = world_state.tick_id;

        info!("[{}-{}] 开始认知流程...", cfg.agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&cfg.persona, tick_id);

        let log_partial_and_fail =
            |chain: &CognitiveChain, stage: &str, err: anyhow::Error| -> anyhow::Error {
                let summary = format!(
                    "认知流程在 {} 阶段失败: {}\n\n已完成的阶段:\n{}",
                    stage, err, chain.summarize()
                );
                thinking_log::log_thinking(&cfg.agent_name, tick_id, &summary);
                err
            };

        // === Stage 1: Perception (带超时) ===
        let perception_prompt =
            self.build_perception_prompt_with_memory(world_state, "", validation_feedback, &cfg);
        let perception_result = timeout(
            Duration::from_secs(STAGE_TIMEOUT_SECS),
            self.perceive_with_memory(world_state, "", validation_feedback, &cfg),
        )
        .await;

        let (perception_response, perception) = match perception_result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(log_partial_and_fail(&chain, "Perception", e)),
            Err(_) => {
                warn!(
                    "[{}-{}] Perception 阶段超时 ({}s)，使用默认感知",
                    cfg.agent_name, tick_id, STAGE_TIMEOUT_SECS
                );
                let default = StageOutput::new(
                    CognitiveStage::Perception,
                    "自身状态正常，环境平静，无特别观察。".to_string(),
                );
                ("{}".to_string(), default)
            }
        };
        chain.add_stage(perception);
        thinking_log::log_llm(
            &cfg.agent_name,
            tick_id,
            "Perception",
            &perception_prompt,
            &perception_response,
        );

        // === Stage 2+3: Motivation + Planning 合并 (带超时) ===
        debug!("执行 Stage 2+3: MotivationPlanning (合并)");
        let perception_output = chain.get_stage(CognitiveStage::Perception).unwrap().clone();
        let mp_prompt = self.build_motivation_planning_prompt(world_state, &perception_output, &cfg);
        let mp_result = timeout(
            Duration::from_secs(STAGE_TIMEOUT_SECS),
            self.motivate_and_plan(world_state, &perception_output, &cfg),
        )
        .await;

        let (motivation, planning) = match mp_result {
            Ok(Ok((mot, plan))) => (mot, plan),
            Ok(Err(e)) => return Err(log_partial_and_fail(&chain, "MotivationPlanning", e)),
            Err(_) => {
                warn!(
                    "[{}-{}] MotivationPlanning 阶段超时 ({}s)，使用默认动机+规划",
                    cfg.agent_name, tick_id, STAGE_TIMEOUT_SECS
                );
                let default_motivation = StageOutput::new(
                    CognitiveStage::Motivation,
                    "主要驱动力: 维持生存 (强度: 5/10)\n原因: 本能驱使".to_string(),
                );
                let default_planning = StageOutput::new(
                    CognitiveStage::Planning,
                    "计划步骤:\n1. 原地休息观察\n预期结果: 保持现状 (优先级: 5/10)".to_string(),
                );
                (default_motivation, default_planning)
            }
        };
        chain.add_stage(motivation);
        chain.add_stage(planning);
        thinking_log::log_llm(
            &cfg.agent_name,
            tick_id,
            "MotivationPlanning",
            &mp_prompt,
            &format!("{{merged: perception={}}}", perception_output.content.len()),
        );

        // === Stage 4: Decision (带超时) ===
        debug!("执行 Stage 4: Decision");
        let decision_prompt = self.build_decision_prompt(world_state, &chain, &cfg);
        let decision_result = timeout(
            Duration::from_secs(STAGE_TIMEOUT_SECS),
            self.decide(world_state, &chain, &cfg),
        )
        .await;

        let (decision_response, decision, intent) = match decision_result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(log_partial_and_fail(&chain, "Decision", e)),
            Err(_) => {
                warn!(
                    "[{}-{}] Decision 阶段超时 ({}s)，回退 idle",
                    cfg.agent_name, tick_id, STAGE_TIMEOUT_SECS
                );
                let agent_id = world_state.agent_id.unwrap_or_default();
                let idle_intent = Intent::new(agent_id, tick_id, "idle", None)
                    .with_thought("决策超时，保守选择休息".to_string());
                let default_decision = StageOutput::new(
                    CognitiveStage::Decision,
                    "思考: 决策超时\n行动: idle".to_string(),
                );
                return Ok({
                    chain.add_stage(default_decision);
                    chain.final_intent = idle_intent;
                    chain.duration_ms = start_time.elapsed().as_millis() as u64;
                    info!(
                        "[{}-{}] 认知完成(含超时回退)，耗时 {}ms",
                        cfg.agent_name, tick_id, chain.duration_ms
                    );
                    thinking_log::log_thinking(&cfg.agent_name, tick_id, &chain.summarize());
                    chain
                });
            }
        };
        chain.add_stage(decision);
        chain.final_intent = intent;
        thinking_log::log_llm(
            &cfg.agent_name,
            tick_id,
            "Decision",
            &decision_prompt,
            &decision_response,
        );

        chain.duration_ms = start_time.elapsed().as_millis() as u64;
        info!(
            "[{}-{}] 认知完成，耗时 {}ms",
            cfg.agent_name, tick_id, chain.duration_ms
        );
        thinking_log::log_thinking(&cfg.agent_name, tick_id, &chain.summarize());

        Ok(chain)
    }

    /// 使用记忆上下文执行完整认知流程（合并动机+规划版）
    pub async fn think_with_memory(
        &self,
        world_state: &WorldState,
        memory_context: &str,
    ) -> Result<CognitiveChain> {
        let start_time = std::time::Instant::now();
        let tick_id = world_state.tick_id;
        let cfg = self.config_snapshot();

        let mut chain = CognitiveChain::from_persona(&cfg.persona, tick_id);

        let log_partial_and_fail =
            |chain: &CognitiveChain, stage: &str, err: anyhow::Error| -> anyhow::Error {
                let summary = format!(
                    "认知流程在 {} 阶段失败: {}\n\n已完成的阶段:\n{}",
                    stage, err, chain.summarize()
                );
                thinking_log::log_thinking(&cfg.agent_name, tick_id, &summary);
                err
            };

        // === Stage 1: Perception (带超时) ===
        let perception_prompt =
            self.build_perception_prompt_with_memory(world_state, memory_context, None, &cfg);
        let perception_result = timeout(
            Duration::from_secs(STAGE_TIMEOUT_SECS),
            self.perceive_with_memory(world_state, memory_context, None, &cfg),
        )
        .await;

        let (perception_response, perception) = match perception_result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(log_partial_and_fail(&chain, "Perception", e)),
            Err(_) => {
                warn!("[{}-{}] Perception 超时，使用默认感知", cfg.agent_name, tick_id);
                let default = StageOutput::new(
                    CognitiveStage::Perception,
                    "自身状态正常，环境平静，无特别观察。".to_string(),
                );
                ("{}".to_string(), default)
            }
        };
        chain.add_stage(perception);
        thinking_log::log_llm(
            &cfg.agent_name, tick_id, "Perception", &perception_prompt, &perception_response,
        );

        // === Stage 2+3: Motivation + Planning 合并 (带超时) ===
        debug!("执行 Stage 2+3: MotivationPlanning (合并)");
        let perception_output = chain.get_stage(CognitiveStage::Perception).unwrap().clone();
        let mp_prompt = self.build_motivation_planning_prompt(world_state, &perception_output, &cfg);
        let mp_result = timeout(
            Duration::from_secs(STAGE_TIMEOUT_SECS),
            self.motivate_and_plan(world_state, &perception_output, &cfg),
        )
        .await;

        let (motivation, planning) = match mp_result {
            Ok(Ok((mot, plan))) => (mot, plan),
            Ok(Err(e)) => return Err(log_partial_and_fail(&chain, "MotivationPlanning", e)),
            Err(_) => {
                warn!("[{}-{}] MotivationPlanning 超时，使用默认", cfg.agent_name, tick_id);
                let default_motivation = StageOutput::new(
                    CognitiveStage::Motivation,
                    "主要驱动力: 维持生存 (强度: 5/10)\n原因: 本能驱使".to_string(),
                );
                let default_planning = StageOutput::new(
                    CognitiveStage::Planning,
                    "计划步骤:\n1. 原地休息观察\n预期结果: 保持现状 (优先级: 5/10)".to_string(),
                );
                (default_motivation, default_planning)
            }
        };
        chain.add_stage(motivation);
        chain.add_stage(planning);
        thinking_log::log_llm(
            &cfg.agent_name, tick_id, "MotivationPlanning", &mp_prompt, "{merged}",
        );

        // === Stage 4: Decision (带超时) ===
        debug!("执行 Stage 4: Decision");
        let decision_prompt = self.build_decision_prompt(world_state, &chain, &cfg);
        let decision_result = timeout(
            Duration::from_secs(STAGE_TIMEOUT_SECS),
            self.decide(world_state, &chain, &cfg),
        )
        .await;

        let (decision_response, decision, intent) = match decision_result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(log_partial_and_fail(&chain, "Decision", e)),
            Err(_) => {
                warn!("[{}-{}] Decision 超时，回退 idle", cfg.agent_name, tick_id);
                let agent_id = world_state.agent_id.unwrap_or_default();
                let idle_intent = Intent::new(agent_id, tick_id, "idle", None)
                    .with_thought("决策超时，保守选择休息".to_string());
                let default_decision = StageOutput::new(
                    CognitiveStage::Decision, "思考: 决策超时\n行动: idle".to_string(),
                );
                chain.add_stage(default_decision);
                chain.final_intent = idle_intent;
                chain.duration_ms = start_time.elapsed().as_millis() as u64;
                info!("[{}-{}] 认知完成(含超时回退)，耗时 {}ms", cfg.agent_name, tick_id, chain.duration_ms);
                thinking_log::log_thinking(&cfg.agent_name, tick_id, &chain.summarize());
                return Ok(chain);
            }
        };
        chain.add_stage(decision);
        chain.final_intent = intent;
        thinking_log::log_llm(
            &cfg.agent_name, tick_id, "Decision", &decision_prompt, &decision_response,
        );

        chain.duration_ms = start_time.elapsed().as_millis() as u64;
        info!("[{}-{}] 认知完成，耗时 {}ms", cfg.agent_name, tick_id, chain.duration_ms);
        thinking_log::log_thinking(&cfg.agent_name, tick_id, &chain.summarize());

        Ok(chain)
    }

    // ========================================================================
    // 各阶段实现
    // ========================================================================

    /// Stage 1: 感知 - 理解当前世界状态
    #[allow(dead_code)]
    async fn perceive(&self, world_state: &WorldState) -> Result<(String, StageOutput)> {
        let cfg = self.config_snapshot();
        let prompt = self.build_perception_prompt(world_state, &cfg);

        let response: PerceptionResponse = self.llm_client.complete_json(&prompt).await?;

        let content = format!(
            "自身状态: {}\n环境: {}\n关键观察: {}",
            response.self_status,
            response.environment,
            response.key_observations.join(", ")
        );

        let metadata = serde_json::to_value(&response)?;
        let response_json = serde_json::to_string(&response)?;
        Ok((
            response_json,
            StageOutput::with_metadata(CognitiveStage::Perception, content, metadata),
        ))
    }

    async fn perceive_with_memory(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
        cfg: &CognitiveEngineConfig,
    ) -> Result<(String, StageOutput)> {
        let prompt = self.build_perception_prompt_with_memory(
            world_state,
            memory_context,
            validation_feedback,
            cfg,
        );

        let response: PerceptionResponse = self.llm_client.complete_json(&prompt).await?;

        let content = format!(
            "自身状态: {}\n环境: {}\n关键观察: {}",
            response.self_status,
            response.environment,
            response.key_observations.join(", ")
        );

        let metadata = serde_json::to_value(&response)?;
        let response_json = serde_json::to_string(&response)?;
        Ok((
            response_json,
            StageOutput::with_metadata(CognitiveStage::Perception, content, metadata),
        ))
    }

    /// Stage 2+3 合并: 动机 + 规划 - 一次 LLM 调用同时生成
    async fn motivate_and_plan(
        &self,
        world_state: &WorldState,
        perception: &StageOutput,
        cfg: &CognitiveEngineConfig,
    ) -> Result<(StageOutput, StageOutput)> {
        let prompt = self.build_motivation_planning_prompt(world_state, perception, cfg);

        let response: MotivationPlanningResponse = self.llm_client.complete_json(&prompt).await?;

        // 拆分为两个 StageOutput
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

        Ok((motivation, planning))
    }

    /// Stage 2: 动机 - 基于人设生成内在驱动力（保留用于向后兼容）
    #[allow(dead_code)]
    async fn motivate(
        &self,
        world_state: &WorldState,
        perception: &StageOutput,
    ) -> Result<(String, StageOutput)> {
        let cfg = self.config_snapshot();
        let prompt = self.build_motivation_prompt(world_state, perception, &cfg);

        let response: MotivationResponse = self.llm_client.complete_json(&prompt).await?;

        let content = format!(
            "主要驱动力: {} (强度: {}/10)\n原因: {}",
            response.primary_drive, response.drive_intensity, response.reasoning
        );

        let metadata = serde_json::to_value(&response)?;
        let response_json = serde_json::to_string(&response)?;
        Ok((
            response_json,
            StageOutput::with_metadata(CognitiveStage::Motivation, content, metadata),
        ))
    }

    /// Stage 3: 规划 - 制定行动计划
    #[allow(dead_code)]
    async fn plan(
        &self,
        world_state: &WorldState,
        perception: &StageOutput,
        motivation: &StageOutput,
    ) -> Result<(String, StageOutput)> {
        let cfg = self.config_snapshot();
        let prompt = self.build_planning_prompt(world_state, perception, motivation, &cfg);

        let response: PlanningResponse = self.llm_client.complete_json(&prompt).await?;

        let content = format!(
            "计划步骤:\n1. {}\n预期结果: {} (优先级: {}/10)",
            response.steps.join("\n2. "),
            response.expected_outcome,
            response.priority
        );

        let metadata = serde_json::to_value(&response)?;
        let response_json = serde_json::to_string(&response)?;
        Ok((
            response_json,
            StageOutput::with_metadata(CognitiveStage::Planning, content, metadata),
        ))
    }

    /// Stage 4: 决策 - 选择最终行动
    async fn decide(
        &self,
        world_state: &WorldState,
        chain: &CognitiveChain,
        cfg: &CognitiveEngineConfig,
    ) -> Result<(String, StageOutput, Intent)> {
        let prompt = self.build_decision_prompt(world_state, chain, cfg);

        let response: DecisionResponse = self.llm_client.complete_json(&prompt).await?;

        let agent_id = world_state.agent_id.unwrap_or_default();
        let tick_id = world_state.tick_id;
        let action_type = response.action.to_lowercase();

        let response_json = serde_json::to_string(&response)?;
        let metadata = serde_json::to_value(&response)?;

        let action_data = if response.action_data.is_null() {
            None
        } else {
            Some(response.action_data)
        };

        let intent = Intent::new(agent_id, tick_id, action_type.as_str(), action_data)
            .with_thought(response.thought_process.clone());

        let content = format!(
            "思考: {}\n行动: {}",
            response.thought_process, intent.action_type
        );

        let stage_output = StageOutput::with_metadata(CognitiveStage::Decision, content, metadata);

        Ok((response_json, stage_output, intent))
    }

    // ========================================================================
    // Prompt 构建方法
    // ========================================================================

    #[allow(dead_code)]
    fn build_perception_prompt(&self, world_state: &WorldState, cfg: &CognitiveEngineConfig) -> String {
        self.build_perception_prompt_with_memory(world_state, "", None, cfg)
    }

    fn build_perception_prompt_with_memory(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
        cfg: &CognitiveEngineConfig,
    ) -> String {
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

        let feedback_section = match validation_feedback {
            Some(fb) => format!("\n[验证反馈]: {}\n", fb),
            None => String::new(),
        };

        format!(
            r#"# 感知阶段 (Perception)
{feedback_section}
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
            agent_name = cfg.agent_name,
            persona = cfg.persona.generate_description(),
            tick_id = world_state.tick_id,
            self_status_section = self_status_section,
            inventory = inventory_str,
            location = world_state.location.name,
            entities = entities_str,
            items = items_str,
            memory_section = memory_section,
            feedback_section = feedback_section,
        )
    }

    #[allow(dead_code)]
    fn build_motivation_prompt(
        &self,
        _world_state: &WorldState,
        perception: &StageOutput,
        cfg: &CognitiveEngineConfig,
    ) -> String {
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
            agent_name = cfg.agent_name,
            persona = cfg.persona.generate_description(),
            perception_content = perception.content
        )
    }

    /// 合并的动机+规划 Prompt（一次 LLM 调用完成两个阶段）
    fn build_motivation_planning_prompt(
        &self,
        _world_state: &WorldState,
        perception: &StageOutput,
        cfg: &CognitiveEngineConfig,
    ) -> String {
        format!(
            r#"# 动机与规划阶段 (Motivation & Planning)

你是 {agent_name}。
{persona}

## 你感知到的
{perception_content}

## 任务
基于你的感知和性格，完成以下两步：
1. 说明你的内在驱动力（你想做什么？为什么？有多强烈？）
2. 制定行动计划（步骤、优先级、预期结果）

## 输出格式
{{
  "primary_drive": "你当前的主要驱动力 (如'获取食物'、'避免危险'、'赚取银两')",
  "drive_intensity": 1-10,
  "reasoning": "为什么有这个动机 (50字以内)",
  "steps": ["步骤1", "步骤2", "..."],
  "priority": 1-10,
  "expected_outcome": "预期结果 (30字以内)"
}}
"#,
            agent_name = cfg.agent_name,
            persona = cfg.persona.generate_description(),
            perception_content = perception.content
        )
    }

    #[allow(dead_code)]
    fn build_planning_prompt(
        &self,
        _world_state: &WorldState,
        perception: &StageOutput,
        motivation: &StageOutput,
        cfg: &CognitiveEngineConfig,
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
            agent_name = cfg.agent_name,
            persona = cfg.persona.generate_description(),
            perception = perception.content,
            motivation = motivation.content
        )
    }

    fn build_decision_prompt(&self, _world_state: &WorldState, chain: &CognitiveChain, cfg: &CognitiveEngineConfig) -> String {
        // 获取各阶段输出作为上下文
        let perception = chain
            .get_stage(CognitiveStage::Perception)
            .map(|s| s.content.as_str())
            .unwrap_or("");
        let motivation = chain
            .get_stage(CognitiveStage::Motivation)
            .map(|s| s.content.as_str())
            .unwrap_or("");
        let planning = chain
            .get_stage(CognitiveStage::Planning)
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
  "action": "动作名称",
  "action_data": {{}}
}}

## 可用动作及 action_data 字段（字段名必须严格匹配，否则服务端会拒绝）

| action | action_data 必填字段 | 说明 |
|--------|---------------------|------|
| idle | (无) | 休息 |
| speak | {{"content": "说的话"}} | 公开说话，所有人可见 |
| move | {{"target_location": "位置名"}} | 移动到指定位置 |
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
            agent_name = cfg.agent_name,
            persona = cfg.persona.generate_description()
        )
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
