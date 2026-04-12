// ============================================================================
// 认知引擎核心（5 阶段，非线性管道）— 人魂 (ActorSoul)
// ============================================================================
//
// 三魂架构中的人魂（行动之魂），负责叙事意图生成：
//   人魂 (ActorSoul)        → 叙事意图（"吃馒头充饥"）
//   天魂 (IntentTranslator)  → 格式化翻译（action_type + action_data with IDs）
//   地魂 (ReflectorSoul)     → 规则/人设审查
//
// 5 个认知阶段通过 2 次合并 LLM 调用执行：
//   1. 感知 + 2. 动机 → LLM Call 1（Perception+Motivation 合并）
//   3. 规划 + 4. 决策 → LLM Call 2（Planning+Decision 合并，输出叙事意图）
//   5a. CognitiveValidator → 认知链质量审查（decision.rs 重试循环内）
//   5b. 天魂 (IntentTranslator) → 叙事→格式化翻译（lifecycle.rs）
//   5c. 地魂 (ReflectorSoul) → 规则/世界观审查（lifecycle.rs）
//
// 人魂不输出结构化 action_data，只输出 narrative_action（自然语言）。
// ID 映射和格式化由天魂负责。
// ============================================================================

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use super::chain::CognitiveChain;
use super::prompt_cache::PromptCache;
use super::stages::{
    CognitiveStage, PerceptionMotivationResponse, PlanDecisionResponse, StageOutput,
};
use super::summary_window::{NarrativeSummary, NarrativeSummaryWindow};
use crate::component::llm::{LlmClient, LlmClientExt};
use crate::component::persona::DynamicPersona;
use crate::infra::api::cognitive_context::load_available_actions_from_file;
use crate::infra::api::thinking_log;
use crate::models::{Intent, WorldEventType, WorldState};
use crate::soul::actor::narrative::{NarrativeEngine, PerceptionNarrative};

use cyber_jianghu_protocol::AvailableAction;

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
///
/// 【Prompt 缓存优化】
/// 使用 PromptCache 实现三层缓存，减少重复内容：
/// - Layer 1: 静态缓存（persona、actions）进程生命周期内不变
/// - Layer 2: 半静态缓存（inventory、locations、entities）变化时更新
/// - Layer 3: 动态状态（self_status、recent_speeches）每轮生成
///
/// 【滑动上下文窗口】
/// 使用 NarrativeSummaryWindow 保留最近 N 轮的行动轨迹摘要，
/// 帮助 LLM 理解连续决策的上下文。
pub struct CognitiveEngine {
    llm_client: Arc<dyn LlmClient>,
    config: std::sync::RwLock<CognitiveEngineConfig>,
    /// Prompt 缓存（分层缓存优化）
    prompt_cache: std::sync::RwLock<PromptCache>,
    /// 滑动上下文窗口（保留最近 N 轮摘要）
    summary_window: std::sync::RwLock<NarrativeSummaryWindow>,
}

impl CognitiveEngine {
    /// 创建新的认知引擎
    pub fn new(llm_client: Arc<dyn LlmClient>, config: CognitiveEngineConfig) -> Self {
        // 初始化 Prompt 缓存
        let persona_desc = config.persona.generate_description();
        let actions_list = Self::load_actions_list();
        let prompt_cache = PromptCache::new(persona_desc, actions_list, &config.persona);

        Self {
            llm_client,
            config: std::sync::RwLock::new(config),
            prompt_cache: std::sync::RwLock::new(prompt_cache),
            // 默认窗口大小为 3
            summary_window: std::sync::RwLock::new(NarrativeSummaryWindow::new(3)),
        }
    }

    /// 使用自定义窗口大小创建认知引擎
    pub fn with_window_size(
        llm_client: Arc<dyn LlmClient>,
        config: CognitiveEngineConfig,
        window_size: usize,
    ) -> Self {
        let persona_desc = config.persona.generate_description();
        let actions_list = Self::load_actions_list();
        let prompt_cache = PromptCache::new(persona_desc, actions_list, &config.persona);

        Self {
            llm_client,
            config: std::sync::RwLock::new(config),
            prompt_cache: std::sync::RwLock::new(prompt_cache),
            summary_window: std::sync::RwLock::new(NarrativeSummaryWindow::new(window_size)),
        }
    }

    /// 加载动作列表（用于缓存）
    fn load_actions_list() -> String {
        let available_actions = load_available_actions_from_file();
        Self::build_action_list(&available_actions)
    }

    /// 使用默认配置创建
    pub fn with_defaults(llm_client: Arc<dyn LlmClient>) -> Self {
        Self::new(llm_client, CognitiveEngineConfig::default())
    }

    /// 更新 Agent 名称（注册新角色后调用）
    pub fn update_agent_name(&self, new_name: &str) {
        let mut config = self.config.write().unwrap();
        config.agent_name = new_name.to_string();
        config.persona.name = new_name.to_string();
        info!("认知引擎 agent_name 已更新: {}", new_name);
    }

    /// 更新 Agent 人设（rebirth 后调用）
    pub fn update_persona(&self, name: &str, system_prompt: &str) {
        let mut config = self.config.write().unwrap();
        config.agent_name = name.to_string();
        config.persona.name = name.to_string();
        config.persona.base_description = system_prompt.to_string();

        // 同步刷新 PromptCache 中的 persona 快照
        let new_desc = config.persona.generate_description();
        let mut cache = self.prompt_cache.write().unwrap();
        cache.invalidate_persona(new_desc, &config.persona);

        info!("认知引擎人设已更新: name={}, prompt_len={}", name, system_prompt.len());
    }

    /// 记录最近一次 action_type（用于行为多样性检测）
    pub fn record_action(&self, action_type: &str) {
        let mut cache = self.prompt_cache.write().unwrap();
        cache.record_action(action_type);
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
        // Extract owned values from config before any .await to keep the future Send-safe.
        let (agent_name, persona) = {
            let cfg = self.config.read().unwrap();
            (cfg.agent_name.clone(), cfg.persona.clone())
        };

        let start_time = std::time::Instant::now();
        let tick_id = world_state.tick_id;

        info!("[{}-{}] 开始认知流程...", agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&persona, tick_id);

        // 【Prompt 缓存优化】
        // - 差异化 persona：第一轮完整，后续摘要
        // - 半静态内容变化时更新缓存
        let persona_for_prompt = {
            let mut cache = self.prompt_cache.write().unwrap();
            cache.check_and_update(world_state);
            cache.get_persona(world_state).to_string()
        };

        // === Stage 1: Perception+Motivation (感知+动机，合并为单次 LLM 调用) ===
        let prompt = self.build_perception_motivation_prompt(
            world_state,
            memory_context,
            validation_feedback,
            &persona_for_prompt,
            &agent_name,
        );
        let (pm_response, perception, motivation) = self.perceive_and_motivate(&prompt).await?;
        chain.add_stage(perception);
        chain.add_stage(motivation);
        thinking_log::log_llm(
            &agent_name,
            tick_id,
            "Perception+Motivation",
            &prompt,
            &pm_response,
        );

        // === Stage 2: Plan+Decide (规划+决策，合并为单次 LLM 调用) ===
        debug!("执行 Stage 2: Plan+Decide");
        let perception_output = chain.get_stage(CognitiveStage::Perception).unwrap().clone();
        let motivation_output = chain.get_stage(CognitiveStage::Motivation).unwrap().clone();

        // 【Prompt 缓存优化】Plan+Decision 也使用差异化的 persona
        let pd_prompt = self.build_plan_decision_prompt(
            &perception_output,
            &motivation_output,
            &persona_for_prompt,
            &agent_name,
        );
        let (pd_response, planning, decision, intent) =
            self.plan_and_decide(&pd_prompt, world_state).await?;
        chain.add_stage(planning);
        chain.add_stage(decision);
        chain.final_intent = intent.clone();
        thinking_log::log_llm(
            &agent_name,
            tick_id,
            "Planning+Decision",
            &pd_prompt,
            &pd_response,
        );

        // 记录耗时
        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        // 【滑动上下文窗口】将结果添加到摘要窗口
        self.push_summary_to_window(&chain, &intent);

        info!(
            "[{}-{}] 认知完成，耗时 {}ms",
            agent_name, tick_id, chain.duration_ms
        );

        thinking_log::log_thinking(&agent_name, tick_id, &chain.summarize());

        Ok(chain)
    }

    /// 将认知结果添加到滑动上下文窗口
    fn push_summary_to_window(&self, chain: &CognitiveChain, intent: &Intent) {
        // 提取叙事意图
        let decision = intent
            .action_data
            .as_ref()
            .and_then(|d| d.get("narrative"))
            .and_then(|n| n.as_str())
            .unwrap_or(&intent.action_type)
            .to_string();

        // 提取感知和动机摘要
        let perception = chain
            .get_stage(CognitiveStage::Perception)
            .map(|s| s.content.chars().take(50).collect())
            .unwrap_or_default();

        let motivation = chain
            .get_stage(CognitiveStage::Motivation)
            .map(|s| s.content.chars().take(50).collect())
            .unwrap_or_default();

        let summary = NarrativeSummary {
            tick_id: chain.tick_id,
            perception,
            motivation,
            decision,
            outcome: "待执行".to_string(), // 结果由外部更新
        };

        self.push_summary(summary);
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
    ///
    /// 人魂只输出叙事意图（narrative_action），不输出结构化 action_data。
    /// 结构化翻译由天魂（IntentTranslator）在 lifecycle.rs 中执行。
    async fn plan_and_decide(
        &self,
        prompt: &str,
        world_state: &WorldState,
    ) -> Result<(String, StageOutput, StageOutput, Intent)> {
        // 人魂只输出叙事意图，不需要 tool calling 查询精确 ID
        // 精确 ID 翻译由天魂（IntentTranslator）负责
        let response: PlanDecisionResponse = self.llm_client.complete_json(prompt).await?;

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

        // Decision stage output: 用 "narrative" 哨兵 action_type 传递叙事意图
        // 天魂（IntentTranslator）在 lifecycle 中检测并翻译
        let agent_id = world_state.agent_id.unwrap_or_default();
        let tick_id = world_state.tick_id;

        let intent = Intent::new(
            agent_id,
            tick_id,
            "narrative",
            Some(serde_json::json!({"narrative": response.narrative_action})),
        )
        .with_thought(response.thought_process.clone());

        let decision_content = format!(
            "思考: {}\n意图: {}",
            response.thought_process, response.narrative_action
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

    /// 构建感知+动机阶段 prompt
    ///
    /// 【Prompt 缓存优化】
    /// - persona_desc：已由调用方从缓存获取（第一轮完整，后续摘要）
    /// - inventory/locations/entities：已由调用方更新到缓存
    /// - 只有 self_status/recent_speeches 每轮生成
    fn build_perception_motivation_prompt(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
        persona_desc: &str,
        agent_name: &str,
    ) -> String {
        let self_state = &world_state.self_state;

        // 【动态状态】每轮生成
        let engine = NarrativeEngine::default();
        let narrative = PerceptionNarrative::from_attributes_with_engine(
            &engine,
            &self_state.attributes,
            &self_state.status_effects,
        );
        let self_status_section = narrative.to_prompt_section();

        // 【半静态缓存】从缓存获取（已在 think_with_memory_and_feedback 中更新）
        let cache = self.prompt_cache.read().unwrap();
        let inventory_str = cache.get_inventory().to_string();
        let adjacent_locations = cache.get_adjacent().to_string();
        let entities_str = cache.get_entities().to_string();
        let items_str = cache.get_nearby_items().to_string();
        let diversity_nudge = cache.get_diversity_nudge().unwrap_or_default();
        drop(cache); // 释放读锁

        // 【Q2: 远端地点提示】独立于缓存逻辑，直接从 WorldState 判断
        let distant_destinations = build_distant_destinations(world_state);

        // 【动态状态】每轮生成
        let memory_section = if memory_context.is_empty() {
            String::new()
        } else {
            format!("\n### 相关记忆\n{memory_context}\n")
        };

        // 从 events_log 中提取最近说过的话和行动反馈（用于去重和反馈）
        let recent_speeches: Vec<String> = world_state
            .events_log
            .iter()
            .rev()
            .filter_map(|e| {
                match e.event_type {
                    WorldEventType::PublicMessage => e
                        .metadata
                        .get("content")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string()),
                    WorldEventType::ActionResult => {
                        // 数据驱动：直接使用 server 提供的 description，不硬编码 action 类型
                        if e.description.is_empty() {
                            None
                        } else {
                            Some(e.description.clone())
                        }
                    }
                    _ => None,
                }
            })
            .take(10)
            .collect();

        let recent_speeches_section = if recent_speeches.is_empty() {
            String::new()
        } else {
            format!(
                "\n### 最近说过的话和对话结果（避免重复）\n{}\n",
                recent_speeches
                    .iter()
                    .map(|s| format!("- {}", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };

        let feedback_section = match validation_feedback {
            Some(fb) => format!("\n[验证反馈]: {}\n", fb),
            None => String::new(),
        };

        // private_dialogue_log: 近期密语索引
        let private_dialogue_section = if world_state.private_dialogue_log.is_empty() {
            String::new()
        } else {
            let entries: Vec<String> = world_state
                .private_dialogue_log
                .iter()
                .map(|d| {
                    format!(
                        "- {} ↔ {} ({}条消息)",
                        d.agent_a_name, d.agent_b_name, d.message_count
                    )
                })
                .collect();
            format!("\n### 近期密语\n{}\n", entries.join("\n"))
        };

        let location_constraint = if world_state.location.adjacent_nodes.is_empty() {
            "\n【重要】当前位置无法移动到任何地方，你必须留在当前位置。"
        } else {
            "\n【重要】只能移动到上述列出的位置，禁止编造或推断其他位置。"
        };

        let time_info = {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let remaining = world_state.deadline_ms.saturating_sub(now_ms) / 1000;
            if remaining > 0 && world_state.deadline_ms > 0 {
                format!(
                    "\n[时间提醒] 当前 Tick 剩余时间约 {} 秒，请在此时间内完成决策。",
                    remaining
                )
            } else {
                String::new()
            }
        };

        // 【滑动上下文窗口】获取近期行动轨迹
        let summary_context = self.get_summary_context();

        format!(
            r#"# 感知与动机阶段 (Perception + Motivation)
{feedback_section}{time_info}
你是 {agent_name}。
{persona}

## 当前游戏状态 (Tick {tick_id})

{self_status_section}
### 背包物品
{inventory}

### 位置
- 地点: {location}
- 可达位置: {adjacent_locations}{location_constraint}{distant_destinations}
### 环境
- 附近的人: {entities}
- 地上的物品: {items}
{diversity_nudge}{memory_section}{recent_speeches_section}{private_dialogue_section}{summary_context}
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
            agent_name = agent_name,
            persona = persona_desc,
            tick_id = world_state.tick_id,
            self_status_section = self_status_section,
            inventory = inventory_str,
            location = world_state.location.name,
            adjacent_locations = adjacent_locations,
            location_constraint = location_constraint,
            distant_destinations = distant_destinations,
            diversity_nudge = diversity_nudge,
            time_info = time_info,
            entities = entities_str,
            items = items_str,
            memory_section = memory_section,
            recent_speeches_section = recent_speeches_section,
            private_dialogue_section = private_dialogue_section,
            summary_context = summary_context,
            feedback_section = feedback_section,
        )
    }

    fn build_plan_decision_prompt(
        &self,
        perception: &StageOutput,
        motivation: &StageOutput,
        persona_desc: &str,
        agent_name: &str,
    ) -> String {
        // 【静态缓存】从缓存获取 actions_list
        let cache = self.prompt_cache.read().unwrap();
        let action_list = cache.get_actions_list().to_string();
        drop(cache);

        format!(
            r#"# 规划与决策阶段 (Planning + Decision)

你是 {agent_name}。
{persona}

## 感知
{perception}

## 动机
{motivation}

## 任务
基于你的感知和动机，制定行动计划并用自然语言描述你想要做的事。
1. 先规划：你打算怎么做？分成几个步骤？
2. 再决策：用一句话描述你最终想做什么（叙事，不需要指定 ID 或格式）。

## 重要约束
1. **必须引用前面的思考**：在 thought_process 中说明你的决策如何基于感知和动机。
2. **不能跳过思考**：必须体现完整的认知链条。
3. **必须以 JSON 格式输出**：不要包含其他文本。
4. **narrative_action 是自然语言**：用你自己的话说想做什么（如"吃一个馒头充饥"、"走到后院看看"、"跟旁边的人打个招呼"）。不要填 ID 或英文字段名。

## 可做之事（参考）
{action_list}

## 输出格式
{{
  "steps": ["步骤1", "步骤2", "..."],
  "priority": 5,
  "expected_outcome": "预期结果 (30字以内)",
  "thought_process": "你的完整思考过程，必须引用感知和动机 (300字以内)",
  "narrative_action": "用自然语言描述你想做的事 (如'吃馒头充饥')"
}}
"#,
            agent_name = agent_name,
            persona = persona_desc,
            perception = perception.content,
            motivation = motivation.content,
            action_list = action_list,
        )
    }

    /// 从动作列表构建简要动作说明（人魂参考用，不含字段细节）
    fn build_action_list(actions: &[AvailableAction]) -> String {
        if actions.is_empty() {
            return "- idle: 休息".to_string();
        }

        actions
            .iter()
            .map(|a| {
                let desc = if a.description.is_empty() {
                    &a.action
                } else {
                    &a.description
                };
                format!("- {}: {}", a.action, desc)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ========================================================================
    // 辅助方法
    // ========================================================================

    /// 添加摘要到滑动窗口
    pub fn push_summary(&self, summary: NarrativeSummary) {
        if let Ok(mut window) = self.summary_window.write() {
            window.push(summary);
        }
    }

    /// 获取滑动窗口上下文（用于 prompt 注入）
    pub fn get_summary_context(&self) -> String {
        if let Ok(window) = self.summary_window.read() {
            window.to_context()
        } else {
            String::new()
        }
    }

    /// 获取详细滑动窗口上下文（用于调试）
    #[allow(dead_code)]
    pub fn get_detailed_summary_context(&self) -> String {
        if let Ok(window) = self.summary_window.read() {
            window.to_detailed_context()
        } else {
            String::new()
        }
    }

    /// 清空滑动窗口
    pub fn clear_summary_window(&self) {
        if let Ok(mut window) = self.summary_window.write() {
            window.clear();
        }
    }

    /// 获取窗口大小
    #[allow(dead_code)]
    pub fn summary_window_size(&self) -> usize {
        if let Ok(window) = self.summary_window.read() {
            window.len()
        } else {
            0
        }
    }
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
                        .with_thought("忽然心神不宁，难以决断，只得暂且静候".to_string())
                    }
                }
            })
        })
    }
}

// ============================================================================
// 远端地点提示 (Q2) — 数据驱动
// ============================================================================

/// 构建远端可达地点提示
///
/// 当 agent 在 sub_scene 时，从 adjacent_nodes 中提取 travel_cost > 1 的节点
/// 作为远端探索目标。数据完全来自 WorldState，无硬编码。
///
/// 限制：sub_scene 的 adjacent_nodes 通常只有 1 跳邻居，
/// 远端目标需要 agent 先移动到 map 级节点后才能看到完整列表。
/// 完整的 2 跳邻居方案需要 Server 协议支持（下发 parent map 的 adjacent_nodes）。
fn build_distant_destinations(world_state: &crate::models::WorldState) -> String {
    // 提取 travel_cost > 1 的相邻节点（远端地点）
    let distant: Vec<_> = world_state
        .location
        .adjacent_nodes
        .iter()
        .filter(|n| n.travel_cost > 1)
        .map(|n| {
            if n.travel_cost > 1 {
                format!("- {} ({}tick)", n.name, n.travel_cost)
            } else {
                n.name.clone()
            }
        })
        .collect();

    if distant.is_empty() {
        return String::new();
    }

    format!(
        "\n### 远端地点\n{}\n",
        distant.join("\n")
    )
}
