// ============================================================================
// 认知引擎核心 — 人魂 (ActorSoul)
// ============================================================================
//
// 人魂直连 WorldState：直接接收客观世界状态，输出结构化 Intent。
// 不再输出叙事中间态（"吃馒头充饥"），直接输出精确 ID（item_id: "mantou"）。
// 天魂翻译步骤已消除。

use anyhow::Result;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

use super::chain::CognitiveChain;
use super::prompt_cache::PromptCache;
use super::stages::CognitiveStage;
use super::summary_window::{NarrativeSummary, NarrativeSummaryWindow};
use crate::component::llm::{LlmClient, LlmClientExt};
use crate::component::persona::DynamicPersona;
use crate::infra::api::cognitive_context::load_available_actions_from_file;
use crate::infra::api::thinking_log;
use crate::models::Intent;

use cyber_jianghu_protocol::{AvailableAction, WorldState};

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

/// 人魂统一认知响应（单次 LLM 调用，直连 WorldState，输出结构化 Intent）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DirectCognitiveResponse {
    /// 状态感知
    self_status: String,
    /// 环境描述
    environment: String,
    /// 关键观察
    key_observations: Vec<String>,
    /// 主要驱动力
    primary_drive: String,
    /// 驱动力强度 (1-10)
    drive_intensity: u8,
    /// 思考过程
    thought_process: String,
    /// 结构化 action_type（如 "eat", "move", "idle"）
    action_type: String,
    /// 结构化 action_data（精确 ID）
    action_data: Option<serde_json::Value>,
}

/// 认知引擎（人魂直连 WorldState）
///
/// 单次 LLM 调用，直接从 WorldState 生成结构化 Intent。
/// Prompt 中包含精确的 item_id、node_id、entity UUID，
/// LLM 直接输出可执行的 Intent（不再走天魂翻译）。
///
/// 【Prompt 缓存优化】
/// 使用 PromptCache 缓存 persona 和 actions，减少重复内容。
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
        let persona_desc = config.persona.generate_description();
        let actions_list = Self::load_actions_list();
        let prompt_cache = PromptCache::new(persona_desc, actions_list, &config.persona);

        Self {
            llm_client,
            config: std::sync::RwLock::new(config),
            prompt_cache: std::sync::RwLock::new(prompt_cache),
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

        let new_desc = config.persona.generate_description();
        let mut cache = self.prompt_cache.write().unwrap();
        cache.invalidate_persona(new_desc, &config.persona);

        info!(
            "认知引擎人设已更新: name={}, prompt_len={}",
            name,
            system_prompt.len()
        );
    }

    // ========================================================================
    // 核心认知方法
    // ========================================================================

    /// 人魂直连 WorldState 认知流程
    ///
    /// 单次 LLM 调用，直接从 WorldState 生成结构化 Intent。
    /// Prompt 包含精确数据（item_id、node_id、entity UUID），
    /// LLM 直接输出 action_type + action_data（不再走天魂翻译）。
    pub async fn think_direct(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        let (agent_name, persona) = {
            let cfg = self.config.read().unwrap();
            (cfg.agent_name.clone(), cfg.persona.clone())
        };
        let tick_id = world_state.tick_id;
        let agent_id = world_state.agent_id.unwrap_or_default();

        let start_time = std::time::Instant::now();
        info!("[{}-{}] 人魂直连认知流程开始...", agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&persona, tick_id);

        let persona_for_prompt = {
            let mut cache = self.prompt_cache.write().unwrap();
            cache.get_persona_simple().to_string()
        };

        let prompt = self.build_direct_prompt(
            world_state,
            memory_context,
            validation_feedback,
            &persona_for_prompt,
            &agent_name,
        );

        let response: DirectCognitiveResponse = self.llm_client.complete_json(&prompt).await?;
        let response_json = serde_json::to_string(&response)?;

        // 构建 CognitiveChain 的 4 个 stage（从统一响应中提取）
        let perception = super::stages::StageOutput::with_metadata(
            CognitiveStage::Perception,
            format!(
                "自身状态: {}\n环境: {}\n关键观察: {}",
                response.self_status,
                response.environment,
                response.key_observations.join(", ")
            ),
            serde_json::json!({
                "self_status": response.self_status,
                "environment": response.environment,
                "key_observations": response.key_observations,
            }),
        );
        chain.add_stage(perception);

        let motivation = super::stages::StageOutput::with_metadata(
            CognitiveStage::Motivation,
            format!(
                "主要驱动力: {} (强度: {}/10)",
                response.primary_drive, response.drive_intensity
            ),
            serde_json::json!({
                "primary_drive": response.primary_drive,
                "drive_intensity": response.drive_intensity,
            }),
        );
        chain.add_stage(motivation);

        let planning = super::stages::StageOutput::with_metadata(
            CognitiveStage::Planning,
            response.thought_process.chars().take(100).collect(),
            serde_json::json!({
                "thought_process": response.thought_process,
            }),
        );
        chain.add_stage(planning);

        // 构建结构化 Intent（直接使用 LLM 输出的 action_type + action_data）
        let intent = Intent::new(
            agent_id,
            tick_id,
            response.action_type.clone(),
            response.action_data.clone(),
        )
        .with_thought(response.thought_process.clone());

        let decision = super::stages::StageOutput::with_metadata(
            CognitiveStage::Decision,
            format!(
                "思考: {}\n决策: {} {:?}",
                response.thought_process, response.action_type, response.action_data
            ),
            serde_json::to_value(&response)?,
        );
        chain.add_stage(decision);
        chain.final_intent = intent.clone();

        thinking_log::log_llm(&agent_name, tick_id, "Direct", &prompt, &response_json);

        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        self.push_summary_to_window(&chain, &intent);

        info!(
            "[{}-{}] 人魂直连认知完成，耗时 {}ms，决策: {}",
            agent_name, tick_id, chain.duration_ms, response.action_type
        );

        thinking_log::log_thinking(&agent_name, tick_id, &chain.summarize());

        Ok(chain)
    }

    /// 旧式认知流程（不接收 WorldState，用于兼容旧回调路径）
    pub async fn think(&self, tick_id: i64, agent_id: Uuid) -> Result<CognitiveChain> {
        self.think_with_feedback(tick_id, agent_id, None).await
    }

    pub async fn think_with_feedback(
        &self,
        tick_id: i64,
        agent_id: Uuid,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        self.think_with_memory_and_feedback(tick_id, agent_id, "", validation_feedback)
            .await
    }

    /// 使用记忆上下文执行认知流程（旧式，用于兼容路径）
    pub async fn think_with_memory(
        &self,
        tick_id: i64,
        agent_id: Uuid,
        memory_context: &str,
    ) -> Result<CognitiveChain> {
        self.think_with_memory_and_feedback(tick_id, agent_id, memory_context, None)
            .await
    }

    /// 旧式核心认知流程（不接收 WorldState，降级路径用）
    pub(crate) async fn think_with_memory_and_feedback(
        &self,
        tick_id: i64,
        agent_id: Uuid,
        memory_context: &str,
        validation_feedback: Option<&str>,
    ) -> Result<CognitiveChain> {
        let (agent_name, persona) = {
            let cfg = self.config.read().unwrap();
            (cfg.agent_name.clone(), cfg.persona.clone())
        };

        let start_time = std::time::Instant::now();
        info!("[{}-{}] 开始认知流程（旧式降级）...", agent_name, tick_id);

        let mut chain = CognitiveChain::from_persona(&persona, tick_id);

        let persona_for_prompt = {
            let mut cache = self.prompt_cache.write().unwrap();
            cache.get_persona_simple().to_string()
        };

        let prompt = self.build_legacy_prompt(
            tick_id,
            memory_context,
            validation_feedback,
            &persona_for_prompt,
            &agent_name,
        );

        let response: DirectCognitiveResponse = self.llm_client.complete_json(&prompt).await?;
        let response_json = serde_json::to_string(&response)?;

        let perception = super::stages::StageOutput::with_metadata(
            CognitiveStage::Perception,
            format!(
                "自身状态: {}\n环境: {}\n关键观察: {}",
                response.self_status,
                response.environment,
                response.key_observations.join(", ")
            ),
            serde_json::json!({
                "self_status": response.self_status,
                "environment": response.environment,
                "key_observations": response.key_observations,
            }),
        );
        chain.add_stage(perception);

        let motivation = super::stages::StageOutput::with_metadata(
            CognitiveStage::Motivation,
            format!(
                "主要驱动力: {} (强度: {}/10)",
                response.primary_drive, response.drive_intensity
            ),
            serde_json::json!({
                "primary_drive": response.primary_drive,
                "drive_intensity": response.drive_intensity,
            }),
        );
        chain.add_stage(motivation);

        let planning = super::stages::StageOutput::with_metadata(
            CognitiveStage::Planning,
            response.thought_process.chars().take(100).collect(),
            serde_json::json!({ "thought_process": response.thought_process }),
        );
        chain.add_stage(planning);

        let intent = Intent::new(
            agent_id,
            tick_id,
            response.action_type.clone(),
            response.action_data.clone(),
        )
        .with_thought(response.thought_process.clone());

        let decision = super::stages::StageOutput::with_metadata(
            CognitiveStage::Decision,
            format!(
                "思考: {}\n决策: {} {:?}",
                response.thought_process, response.action_type, response.action_data
            ),
            serde_json::to_value(&response)?,
        );
        chain.add_stage(decision);
        chain.final_intent = intent.clone();

        thinking_log::log_llm(&agent_name, tick_id, "Legacy", &prompt, &response_json);

        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        self.push_summary_to_window(&chain, &intent);

        info!(
            "[{}-{}] 旧式认知完成，耗时 {}ms",
            agent_name, tick_id, chain.duration_ms
        );

        thinking_log::log_thinking(&agent_name, tick_id, &chain.summarize());

        Ok(chain)
    }

    // ========================================================================
    // Prompt 构建方法
    // ========================================================================

    /// 构建直连 WorldState 的 prompt（包含精确数据）
    fn build_direct_prompt(
        &self,
        world_state: &WorldState,
        memory_context: &str,
        validation_feedback: Option<&str>,
        persona_desc: &str,
        agent_name: &str,
    ) -> String {
        let feedback_section = match validation_feedback {
            Some(fb) => format!("\n[验证反馈]: {}\n", fb),
            None => String::new(),
        };

        let memory_section = if memory_context.is_empty() {
            String::new()
        } else {
            format!("\n### 记忆上下文\n{memory_context}\n")
        };

        let summary_context = self.get_summary_context();

        let cache = self.prompt_cache.read().unwrap();
        let action_list = cache.get_actions_list().to_string();
        drop(cache);

        // 从 WorldState 构建精确数据段
        let mut ws_parts = Vec::new();

        ws_parts.push(format!("- Tick: {}", world_state.tick_id));
        ws_parts.push(format!(
            "- 位置: {} ({})",
            world_state.location.name, world_state.location.node_id
        ));
        ws_parts.push(format!("- 时间: {}", world_state.world_time.to_chinese()));

        // 自身属性描述（叙事化）
        if !world_state.self_state.attribute_descriptions.is_empty() {
            ws_parts.push("\n## 自身状态".to_string());
            for (attr, desc) in &world_state.self_state.attribute_descriptions {
                ws_parts.push(format!("- {}: {}", attr, desc));
            }
        }

        // 背包物品（精确 item_id）
        if !world_state.self_state.inventory.is_empty() {
            ws_parts.push("\n## 背包物品".to_string());
            for item in &world_state.self_state.inventory {
                ws_parts.push(format!(
                    "- {} ({}) x{}",
                    item.item_id, item.name, item.quantity
                ));
            }
        }

        // 附近物品（精确 item_id）
        if !world_state.nearby_items.is_empty() {
            ws_parts.push("\n## 附近可见物品".to_string());
            for item in &world_state.nearby_items {
                ws_parts.push(format!(
                    "- {} ({}) x{}",
                    item.item_id, item.name, item.quantity
                ));
            }
        }

        // 附近 Agent（精确 UUID）
        if !world_state.entities.is_empty() {
            ws_parts.push("\n## 附近的人".to_string());
            for entity in &world_state.entities {
                ws_parts.push(format!("- {} (UUID: {})", entity.name, entity.id));
            }
        }

        // 相邻地点（精确 node_id）
        if !world_state.location.adjacent_nodes.is_empty() {
            ws_parts.push("\n## 可前往的地点".to_string());
            for node in &world_state.location.adjacent_nodes {
                ws_parts.push(format!("- {} ({})", node.name, node.node_id));
            }
        }

        // 可采集资源
        if !world_state.location.gatherable_items.is_empty() {
            ws_parts.push("\n## 当前位置可采集的资源".to_string());
            for item in &world_state.location.gatherable_items {
                ws_parts.push(format!("- {} ({})", item.name, item.item_id));
            }
        }

        // 事件日志
        if !world_state.events_log.is_empty() {
            ws_parts.push("\n## 近期事件".to_string());
            for event in &world_state.events_log {
                ws_parts.push(format!("- {}", event.description));
            }
        }

        let world_state_section = ws_parts.join("\n");

        format!(
            r#"{feedback_section}你是 {agent_name}。
{persona}

## 当前世界状态
{world_state_section}
{memory_section}
{summary_context}
## 任务
基于你的性格和当前状态，做出决策。你直接输出结构化 Intent，包含精确的 ID。

## 生存法则
- 饥饿或口渴严重时，进食/饮水是最高优先级
- 没有食物时：先拾取地上的食物/水（pickup），再进食/饮水（eat/drink）
- 背包和地面都没有时：移动到可能有资源的地点（move）
- idle（原地休息）是合法行为，不必强求每个 tick 都行动

## 可做之事（参考）
{action_list}

## 输出格式
严格输出以下 JSON（不要添加任何额外文本）：
{{
  "self_status": "你的状态简述 (30字以内)",
  "environment": "环境描述 (30字以内)",
  "key_observations": ["观察1", "观察2"],
  "primary_drive": "当前主要驱动力",
  "drive_intensity": 5,
  "thought_process": "完整思考过程 (200字以内)",
  "action_type": "动作类型（如 eat, drink, pickup, move, idle, speak 等）",
  "action_data": {{}}
}}

### action_type 与 action_data 对应关系：
- idle: {{"action_data": null}}
- eat: {{"action_data": {{"item_id": "背包中的食物item_id"}}}}
- drink: {{"action_data": {{"item_id": "背包中的饮品item_id"}}}}
- pickup: {{"action_data": {{"item_id": "地上物品的item_id"}}}}
- move: {{"action_data": {{"target_location": "目标地点的node_id"}}}}
- speak: {{"action_data": {{"content": "你想说的话", "target_agent_id": "对方UUID（可选）"}}}}
- give: {{"action_data": {{"item_id": "物品ID", "target_agent_id": "对方UUID"}}}}

注意：action_data 中的 ID 必须从上面的世界状态数据中直接复制，不要编造。"#,
            agent_name = agent_name,
            persona = persona_desc,
            world_state_section = world_state_section,
            memory_section = memory_section,
            summary_context = summary_context,
            feedback_section = feedback_section,
            action_list = action_list,
        )
    }

    /// 构建旧式 prompt（不接收 WorldState，降级路径）
    fn build_legacy_prompt(
        &self,
        tick_id: i64,
        memory_context: &str,
        validation_feedback: Option<&str>,
        persona_desc: &str,
        agent_name: &str,
    ) -> String {
        let feedback_section = match validation_feedback {
            Some(fb) => format!("\n[验证反馈]: {}\n", fb),
            None => String::new(),
        };

        let memory_section = if memory_context.is_empty() {
            String::new()
        } else {
            format!("\n### 当前状态与感知\n{memory_context}\n")
        };

        let summary_context = self.get_summary_context();

        let cache = self.prompt_cache.read().unwrap();
        let action_list = cache.get_actions_list().to_string();
        drop(cache);

        format!(
            r#"{feedback_section}你是 {agent_name}。
{persona}

## 当前游戏状态 (Tick {tick_id})
{memory_section}
{summary_context}
## 任务
基于你的性格和当前状态，做出决策。

## 生存法则
- 饥饿或口渴严重时，进食/饮水是最高优先级
- 没有食物时：先拾取地上的食物/水
- idle（原地休息）是合法行为

## 可做之事（参考）
{action_list}

## 输出格式
严格输出以下 JSON：
{{
  "self_status": "你的状态简述 (30字以内)",
  "environment": "环境描述 (30字以内)",
  "key_observations": ["观察1", "观察2"],
  "primary_drive": "当前主要驱动力",
  "drive_intensity": 5,
  "thought_process": "完整思考过程 (200字以内)",
  "action_type": "动作类型",
  "action_data": {{}}
}}"#,
            tick_id = tick_id,
            agent_name = agent_name,
            persona = persona_desc,
            memory_section = memory_section,
            summary_context = summary_context,
            feedback_section = feedback_section,
            action_list = action_list,
        )
    }

    /// 从动作列表构建简要动作说明
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
    // 滑动上下文窗口
    // ========================================================================

    /// 将认知结果添加到滑动上下文窗口
    fn push_summary_to_window(&self, chain: &CognitiveChain, intent: &Intent) {
        let decision = intent.action_type.as_str().to_string();

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
            outcome: "待执行".to_string(),
        };

        self.push_summary(summary);
    }

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
    /// 创建决策回调（兼容旧接口，不接收 WorldState）
    pub fn create_decision_callback(self) -> crate::runtime::DecisionCallback {
        let engine = Arc::new(self);
        Arc::new(move |tick_id: i64, agent_id: uuid::Uuid| {
            let engine = engine.clone();
            Box::pin(async move {
                match engine.think(tick_id, agent_id).await {
                    Ok(chain) => chain.final_intent,
                    Err(e) => {
                        tracing::error!("多阶段认知失败: {}", e);
                        Intent::new(agent_id, tick_id, "idle", None)
                            .with_thought("忽然心神不宁，难以决断，只得暂且静候".to_string())
                    }
                }
            })
        })
    }
}
