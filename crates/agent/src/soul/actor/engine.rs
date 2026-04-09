// ============================================================================
// 认知引擎核心（5 阶段，非线性管道）
// ============================================================================
//
// 5 个认知阶段通过 2 次合并 LLM 调用执行，降低 token 消耗和延迟：
//   1. 感知 + 2. 动机 → LLM Call 1（Perception+Motivation 合并）
//   3. 规划 + 4. 决策 → LLM Call 2（Planning+Decision 合并）
//   5a. CognitiveValidator → 认知链质量审查（本文件/decision.rs 重试循环内，5 条规则）
//   5b. ReflectorSoul → 规则/道德审查（engine 外部，lifecycle.rs）
//
// Prompt 注入上下文：
//   - 背包/地面物品: "name [item_id] xN" 格式，确保 LLM 使用系统 ID
//   - 可达位置: "name [node_id]" 格式，同上
//   - 最近发言: 从 events_log 提取最近 5 条 public_message，用于去重
//   - 动作表: 含 dialogue 私聊动作，item_id/target_location 强制使用英文 ID
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
use crate::infra::api::cognitive_context::load_available_actions_from_file;
use crate::infra::api::thinking_log;
use crate::models::{Intent, WorldEventType, WorldState};
use crate::soul::actor::narrative::{NarrativeEngine, PerceptionNarrative};
use crate::soul::actor::tools::{ActorToolExecutor, create_actor_tools};
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
pub struct CognitiveEngine {
    llm_client: Arc<dyn LlmClient>,
    config: std::sync::RwLock<CognitiveEngineConfig>,
}

impl CognitiveEngine {
    /// 创建新的认知引擎
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

    /// 更新 Agent 名称（注册新角色后调用）
    pub fn update_agent_name(&self, new_name: &str) {
        let mut config = self.config.write().unwrap();
        config.agent_name = new_name.to_string();
        config.persona.name = new_name.to_string();
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

        // 缓存 persona description（同一 tick 内人设不变）
        let persona_desc = persona.generate_description();

        // === Stage 1: Perception+Motivation (感知+动机，合并为单次 LLM 调用) ===
        let prompt = self.build_perception_motivation_prompt(
            world_state,
            memory_context,
            validation_feedback,
            &persona_desc,
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
        let pd_prompt = self.build_plan_decision_prompt(
            &perception_output,
            &motivation_output,
            &persona_desc,
            &agent_name,
        );
        let (pd_response, planning, decision, intent) =
            self.plan_and_decide(&pd_prompt, world_state).await?;
        chain.add_stage(planning);
        chain.add_stage(decision);
        chain.final_intent = intent;
        thinking_log::log_llm(
            &agent_name,
            tick_id,
            "Planning+Decision",
            &pd_prompt,
            &pd_response,
        );

        // 记录耗时
        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        info!(
            "[{}-{}] 认知完成，耗时 {}ms",
            agent_name, tick_id, chain.duration_ms
        );

        thinking_log::log_thinking(&agent_name, tick_id, &chain.summarize());

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
        let mut response: PlanDecisionResponse = if self.llm_client.supports_tool_calling() {
            let tools = create_actor_tools();
            let executor = ActorToolExecutor::new(world_state.clone());
            match self
                .llm_client
                .complete_json_with_tools("", prompt, &tools, &executor, 1)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Tool calling failed, fallback to direct: {}", e);
                    self.llm_client.complete_json(prompt).await?
                }
            }
        } else {
            self.llm_client.complete_json(prompt).await?
        };

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
        agent_name: &str,
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
                .map(|i| format!("{} ({}): {} 个", i.item_id, i.name, i.quantity))
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
                .map(|i| format!("{} ({}): {} 个", i.item_id, i.name, i.quantity))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let memory_section = if memory_context.is_empty() {
            String::new()
        } else {
            format!("\n### 相关记忆\n{memory_context}\n")
        };

        // 从events_log中提取最近说过的话和行动反馈（用于去重和反馈）
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
            let entries: Vec<String> = world_state.private_dialogue_log.iter()
                .map(|d| format!("- {} ↔ {} ({}条消息)", d.agent_a_name, d.agent_b_name, d.message_count))
                .collect();
            format!("\n### 近期密语\n{}\n", entries.join("\n"))
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

        let location_constraint = if world_state.location.adjacent_nodes.is_empty() {
            "\n【重要】当前位置无法移动到任何地方，你必须留在当前位置。"
        } else {
            "\n【重要】只能移动到上述明确列出的位置，禁止编造或推断其他位置名称。"
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
- 可达位置: {adjacent_locations}{location_constraint}

### 环境
- 附近的人: {entities}
- 地上的物品: {items}
{memory_section}{recent_speeches_section}{private_dialogue_section}
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
            time_info = time_info,
            entities = entities_str,
            items = items_str,
            memory_section = memory_section,
            recent_speeches_section = recent_speeches_section,
            private_dialogue_section = private_dialogue_section,
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
        // 从本地文件加载动作表
        let available_actions = load_available_actions_from_file();
        let dynamic_action_table = Self::build_action_table(&available_actions);

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

## 工具使用
你可以调用工具查询精确的游戏数据。当你决定涉及物品或地点的行动时，请先调用工具确认正确的 ID。
工具返回的数据是绝对权威的，你必须使用工具返回的精确 ID，不要自行翻译或猜测。

## 重要约束
1. **必须引用前面的思考**：在 thought_process 中说明你的决策如何基于感知和动机。
2. **不能跳过思考**：必须体现完整的认知链条。
3. **必须以 JSON 格式输出**：不要包含其他文本。

!!! 生死攸关的 ID 规则（违反必死）!!!

物品、位置、采集目标必须使用英文字母的 ID，绝不能用括号内的中文名称：
- 背包格式: "mantou (馒头): 3 个" — 用 "mantou"，不要用 "馒头"
- 使用物品: item_id 填 "water" 而非 "水" → {{"item_id": "water"}}
- 移动: target_location 填 "longmen_backyard" 而非 "后院" → {{"target_location": "longmen_backyard"}}
- 采集: target_id 填 "water" 而非 "老井" → {{"target_id": "water"}}

记住：英文字母开头的才是 ID。用中文名称 = 动作失败 = 资源耗尽 = 死亡。

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

{dynamic_action_table}

target_agent_id 从 entities 列表中的 agent_id 获取。
"#,
            agent_name = agent_name,
            persona = persona_desc,
            perception = perception.content,
            motivation = motivation.content,
            dynamic_action_table = dynamic_action_table,
        )
    }

    /// 从动作列表构建动作表
    fn build_action_table(actions: &[AvailableAction]) -> String {
        if actions.is_empty() {
            return "| idle | (无) | 休息 |".to_string();
        }

        let mut table = String::from(
            "| action | action_data 必填字段 | 说明 |\n|--------|---------------------|------|\n",
        );

        for action in actions {
            let fields = if action.required_fields.is_empty() {
                "(无)".to_string()
            } else {
                action
                    .required_fields
                    .iter()
                    .map(|f| format!("\"{}\"", f))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let desc = if action.description.is_empty() {
                &action.action
            } else {
                &action.description
            };
            table.push_str(&format!(
                "| {} | {{{}}} | {} |\n",
                action.action, fields, desc
            ));
        }

        table
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
                        .with_thought("忽然心神不宁，难以决断，只得暂且静候".to_string())
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
