// ============================================================================
// 认知引擎核心 — 人魂 (ActorSoul)
// ============================================================================
//
// 人魂直连 WorldState：直接接收客观世界状态，输出结构化 Intent。
// 不再输出叙事中间态（"吃馒头充饥"），直接输出精确 ID（item_id: "mantou"）。
// 天魂翻译步骤已消除。

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

use super::chain::CognitiveChain;
use super::prompt_cache::PromptCache;
use super::prompt_template::PromptTemplateConfig;
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

/// 单个结构化 action
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct DirectCognitiveAction {
    /// 结构化 action_type（如 "eat", "move", "idle"）
    pub action_type: String,
    /// 结构化 action_data（精确 ID）
    pub action_data: Option<serde_json::Value>,
}

/// 人魂统一认知响应（单次 LLM 调用，直连 WorldState，输出结构化 Intent）
///
/// 支持两种 LLM 输出格式（向后兼容）：
/// - 新格式: `actions: [{action_type, action_data}, ...]` — 1-3 个 sequential actions
/// - 旧格式: `action_type + action_data` — 单个 action（自动转换为 actions 数组）
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
    /// 多 action 格式（新）
    #[serde(default)]
    actions: Vec<DirectCognitiveAction>,
    /// 单 action 格式（旧，向后兼容）
    #[serde(default)]
    action_type: Option<String>,
    /// 单 action_data 格式（旧，向后兼容）
    #[serde(default)]
    action_data: Option<serde_json::Value>,
}

impl DirectCognitiveResponse {
    /// 统一获取 actions 列表
    ///
    /// 优先使用 `actions` 字段（新格式），fallback 到 `action_type` + `action_data`（旧格式）。
    fn get_actions(&self) -> Vec<DirectCognitiveAction> {
        if !self.actions.is_empty() {
            return self.actions.clone();
        }
        // 旧格式 fallback
        if let Some(ref at) = self.action_type {
            vec![DirectCognitiveAction {
                action_type: at.clone(),
                action_data: self.action_data.clone(),
            }]
        } else {
            vec![DirectCognitiveAction {
                action_type: "idle".to_string(),
                action_data: None,
            }]
        }
    }
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
    /// Prompt 模板配置（从 YAML 加载，None 时 fail-fast）
    prompt_template: Option<PromptTemplateConfig>,
}

impl CognitiveEngine {
    /// 创建新的认知引擎
    pub fn new(llm_client: Arc<dyn LlmClient>, config: CognitiveEngineConfig) -> Self {
        let persona_desc = config.persona.generate_description();
        let (action_descriptions, action_field_hints) = Self::load_actions_list();
        let prompt_cache = PromptCache::new(
            persona_desc,
            action_descriptions,
            action_field_hints,
            &config.persona,
        );

        let prompt_template = Self::load_prompt_template();

        Self {
            llm_client,
            config: std::sync::RwLock::new(config),
            prompt_cache: std::sync::RwLock::new(prompt_cache),
            summary_window: std::sync::RwLock::new(NarrativeSummaryWindow::new(3)),
            prompt_template,
        }
    }

    /// 使用自定义窗口大小创建认知引擎
    pub fn with_window_size(
        llm_client: Arc<dyn LlmClient>,
        config: CognitiveEngineConfig,
        window_size: usize,
    ) -> Self {
        let persona_desc = config.persona.generate_description();
        let (action_descriptions, action_field_hints) = Self::load_actions_list();
        let prompt_cache = PromptCache::new(
            persona_desc,
            action_descriptions,
            action_field_hints,
            &config.persona,
        );

        let prompt_template = Self::load_prompt_template();

        Self {
            llm_client,
            config: std::sync::RwLock::new(config),
            prompt_cache: std::sync::RwLock::new(prompt_cache),
            summary_window: std::sync::RwLock::new(NarrativeSummaryWindow::new(window_size)),
            prompt_template,
        }
    }

    /// 加载 prompt 模板配置
    ///
    /// 查找路径：
    /// 1. $CYBER_JIANGHU_CONFIG_DIR/prompt_templates.yaml
    /// 2. ~/.cyber-jianghu/config/prompt_templates.yaml
    /// 3. 内置默认路径（编译时嵌入或同级 config/）
    ///
    /// Fail-fast: 配置文件存在但格式错误时 panic。
    /// 不存在时使用硬编码模板（向后兼容旧部署）。
    fn load_prompt_template() -> Option<PromptTemplateConfig> {
        let search_paths = [
            std::env::var("CYBER_JIANGHU_CONFIG_DIR")
                .ok()
                .map(|d| std::path::PathBuf::from(d).join("prompt_templates.yaml")),
            dirs::home_dir().map(|h| {
                h.join(".cyber-jianghu")
                    .join("config")
                    .join("prompt_templates.yaml")
            }),
            Some(std::path::PathBuf::from("config/prompt_templates.yaml")),
        ];

        for path_opt in &search_paths {
            if let Some(path) = path_opt
                && path.exists()
            {
                match PromptTemplateConfig::load_from_file(path) {
                    Ok(config) => {
                        info!("已加载 prompt 模板: {:?}", path);
                        return Some(config);
                    }
                    Err(e) => {
                        panic!("Prompt 模板文件格式错误 ({}): {}", path.display(), e);
                    }
                }
            }
        }
        info!("未找到 prompt_templates.yaml，使用内置模板");
        None
    }

    /// 获取 Prompt 模板配置的引用
    pub fn prompt_template(&self) -> Option<&PromptTemplateConfig> {
        self.prompt_template.as_ref()
    }

    /// 获取截断长度配置（数据驱动替代 .take(N) 魔法数字）
    fn truncation(&self, key: &str, default: usize) -> usize {
        self.prompt_template
            .as_ref()
            .map(|c| c.truncation("actor_direct", key, default))
            .unwrap_or(default)
    }

    /// 加载动作列表（用于缓存）
    fn load_actions_list() -> (String, String) {
        let available_actions = load_available_actions_from_file();
        let descriptions = Self::build_action_descriptions(&available_actions);
        let field_hints = Self::build_action_field_hints(&available_actions);
        (descriptions, field_hints)
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
            response
                .thought_process
                .chars()
                .take(self.truncation("planning_description", 100))
                .collect(),
            serde_json::json!({
                "thought_process": response.thought_process,
            }),
        );
        chain.add_stage(planning);

        // 构建结构化 Intents（从 actions 数组，向后兼容旧格式）
        let actions = response.get_actions();
        let intents: Vec<Intent> = actions
            .iter()
            .map(|a| {
                Intent::new(agent_id, tick_id, a.action_type.clone(), a.action_data.clone())
                    .with_thought(response.thought_process.clone())
            })
            .collect();

        let primary_action = &actions[0];
        let decision = super::stages::StageOutput::with_metadata(
            CognitiveStage::Decision,
            format!(
                "思考: {}\n决策: {} {:?}{}",
                response.thought_process,
                primary_action.action_type,
                primary_action.action_data,
                if actions.len() > 1 {
                    format!(" (+{} 后续)", actions.len() - 1)
                } else {
                    String::new()
                }
            ),
            serde_json::to_value(&response)?,
        );
        chain.add_stage(decision);
        chain.final_intent = intents[0].clone();

        thinking_log::log_llm(&agent_name, tick_id, "Direct", &prompt, &response_json);

        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        self.push_summary_to_window(&chain, &intents[0]);

        info!(
            "[{}-{}] 人魂直连认知完成，耗时 {}ms，决策: {} ({} 个 action)",
            agent_name, tick_id, chain.duration_ms, primary_action.action_type, intents.len()
        );

        thinking_log::log_thinking(&agent_name, tick_id, &chain.summarize());

        // 将 multi-intent 存入 chain metadata 供 lifecycle 读取
        chain.multi_intents = if intents.len() > 1 {
            Some(intents[1..].to_vec())
        } else {
            None
        };

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
            response
                .thought_process
                .chars()
                .take(self.truncation("planning_description", 100))
                .collect(),
            serde_json::json!({ "thought_process": response.thought_process }),
        );
        chain.add_stage(planning);

        // 旧式路径也支持多 action 格式
        let actions = response.get_actions();
        let intent = Intent::new(
            agent_id,
            tick_id,
            actions[0].action_type.clone(),
            actions[0].action_data.clone(),
        )
        .with_thought(response.thought_process.clone());

        let decision = super::stages::StageOutput::with_metadata(
            CognitiveStage::Decision,
            format!(
                "思考: {}\n决策: {} {:?}",
                response.thought_process, actions[0].action_type, actions[0].action_data
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
        let action_descriptions = cache.get_action_descriptions().to_string();
        let action_field_hints = cache.get_action_field_hints().to_string();
        drop(cache);

        // 从 WorldState 构建精确数据段
        let world_state_section = self.build_world_state_section(world_state);

        // 尝试使用模板配置
        if let Some(ref template_config) = self.prompt_template
            && let Some(tmpl) = template_config.get_template("actor_direct")
        {
            let mut vars = HashMap::new();
            vars.insert("feedback_section".to_string(), feedback_section);
            vars.insert("agent_name".to_string(), agent_name.to_string());
            vars.insert("persona".to_string(), persona_desc.to_string());
            vars.insert("world_state_section".to_string(), world_state_section);
            vars.insert("memory_section".to_string(), memory_section);
            vars.insert("summary_context".to_string(), summary_context);
            vars.insert("action_descriptions".to_string(), action_descriptions);
            vars.insert("action_field_hints".to_string(), action_field_hints);

            return tmpl.render_all(&vars);
        }

        // 模板不可用时的内置模板（向后兼容旧部署）
        self.build_hardcoded_prompt(
            &feedback_section,
            agent_name,
            persona_desc,
            &world_state_section,
            &memory_section,
            &summary_context,
            &action_descriptions,
            &action_field_hints,
        )
    }

    /// 构建 WorldState 数据段（共享逻辑，模板和硬编码路径共用）
    fn build_world_state_section(&self, world_state: &WorldState) -> String {
        let content_hint_len = self
            .prompt_template
            .as_ref()
            .and_then(|t| t.templates.get("actor_direct"))
            .and_then(|t| t.truncation.get("content_hint"))
            .copied()
            .unwrap_or(30);

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
                let raw = world_state
                    .self_state
                    .attributes
                    .get(attr)
                    .map(|v| format!(" [当前值: {}]", v))
                    .unwrap_or_default();
                ws_parts.push(format!("- {}: {}{}", attr, desc, raw));
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

        // 附近 Agent（精确 UUID + 近期动作）
        if !world_state.entities.is_empty() {
            ws_parts.push("\n## 附近的人".to_string());
            for entity in &world_state.entities {
                ws_parts.push(format!("- {} (UUID: {})", entity.name, entity.id));
                for action in &entity.recent_actions {
                    let content_hint = action
                        .content
                        .as_ref()
                        .map(|c| {
                            let truncated: String = c.chars().take(content_hint_len).collect();
                            format!("「{}」", truncated)
                        })
                        .unwrap_or_default();
                    ws_parts.push(format!(
                        "  [Tick {}] {} {}{}",
                        action.tick_id, action.action_type, action.result, content_hint
                    ));
                }
            }
        }

        // 当前位置 + 可前往地点（强化地点约束）
        ws_parts.push(format!(
            "\n## 当前位置：{} ({})",
            world_state.location.name, world_state.location.node_id
        ));
        if !world_state.location.adjacent_nodes.is_empty() {
            ws_parts.push("## 可前往的地点（仅这些地点存在）".to_string());
            for node in &world_state.location.adjacent_nodes {
                ws_parts.push(format!(
                    "- {} ({})，移动消耗：{} tick",
                    node.name, node.node_id, node.travel_cost
                ));
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

        ws_parts.join("\n")
    }

    /// 内置硬编码 prompt（向后兼容旧部署）
    #[allow(clippy::too_many_arguments)]
    fn build_hardcoded_prompt(
        &self,
        feedback_section: &str,
        agent_name: &str,
        persona_desc: &str,
        world_state_section: &str,
        memory_section: &str,
        summary_context: &str,
        action_descriptions: &str,
        action_field_hints: &str,
    ) -> String {
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
- 当饥饿或口渴描述中出现紧迫措辞时（如"饥肠辘辘/饥饿难耐/急需"等），进食/饮水是最高优先级
- 没有食物时：先拾取地上的食物/水（pickup），再进食/饮水（eat/drink）
- 背包和地面都没有时：采集（gather）或移动到可能有资源的地点（move）
- idle（原地休息）是合法行为，不必强求每个 tick 都行动
- eat/drink 的 action_data 必须使用"背包物品"或"附近物品"中列出的精确 item_id，禁止使用物品名称或自创 ID

## 叙事限制
- 叙事只能引用"背包物品"或"附近可见物品"中确实存在的物品
- 不得描述其他角色的行为，除非"附近的人"中有该角色的近期动作记录
- 不得与不在"附近的人"列表中的角色互动
- **世界地图仅由"可前往的地点"定义。不存在其他地点。不得在 thought_process 或 environment 中提及未列出的地点**
- 不得编造未发生的事件（如劫镖、打斗、天灾），除非"近期事件"中有记录
- 如果对某事没有观察证据，thought_process 中应标注[未确认]

## 可做之事（参考）
{action_descriptions}

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

### action_data 字段要求：
{action_field_hints}

注意：action_data 中的 ID 必须从上面的世界状态数据中直接复制，不要编造。"#,
            agent_name = agent_name,
            persona = persona_desc,
            world_state_section = world_state_section,
            memory_section = memory_section,
            summary_context = summary_context,
            feedback_section = feedback_section,
            action_descriptions = action_descriptions,
            action_field_hints = action_field_hints,
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

    /// 从动作列表构建动作描述（"可做之事"部分，含语义说明）
    fn build_action_descriptions(actions: &[AvailableAction]) -> String {
        if actions.is_empty() {
            return "- idle: 休息".to_string();
        }

        actions
            .iter()
            .map(|a| {
                let desc = if a.description.is_empty() {
                    a.name.clone()
                } else {
                    a.description.clone()
                };
                format!("- {}: {}", a.action, desc)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 从动作列表构建字段 schema（"action_data 字段要求"部分）
    fn build_action_field_hints(actions: &[AvailableAction]) -> String {
        if actions.is_empty() {
            return "- idle: (action_data: null)".to_string();
        }

        actions
            .iter()
            .map(|a| {
                let fields_hint = if a.required_fields.is_empty() {
                    "(action_data: null)".to_string()
                } else {
                    let fields_str = a
                        .required_fields
                        .iter()
                        .map(|f| format!("\"{}\": ...", f))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("(action_data: {{ {} }})", fields_str)
                };
                format!("- {}: {}", a.action, fields_hint)
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
            .map(|s| {
                s.content
                    .chars()
                    .take(self.truncation("summary_window", 50))
                    .collect()
            })
            .unwrap_or_default();

        let motivation = chain
            .get_stage(CognitiveStage::Motivation)
            .map(|s| {
                s.content
                    .chars()
                    .take(self.truncation("summary_window", 50))
                    .collect()
            })
            .unwrap_or_default();

        let summary = NarrativeSummary {
            tick_id: chain.tick_id,
            perception,
            motivation,
            decision,
            outcome: "执行中".to_string(),
        };

        self.push_summary(summary);
    }

    /// 添加摘要到滑动窗口
    pub fn push_summary(&self, summary: NarrativeSummary) {
        if let Ok(mut window) = self.summary_window.write() {
            window.push(summary);
        }
    }

    /// 更新最近一条摘要的 outcome（Intent 执行结果写回）
    pub fn update_summary_outcome(&self, outcome: String) {
        if let Ok(mut window) = self.summary_window.write() {
            window.update_last_outcome(outcome);
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
