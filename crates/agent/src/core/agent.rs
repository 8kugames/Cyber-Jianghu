// ============================================================================
// Agent 核心
// ============================================================================
//
// Agent 结构定义和基本方法
// ============================================================================

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast};
use tracing::{info, warn};
use uuid::Uuid;

use crate::component::dialogue::DialogueContextManager;
use crate::component::immediate::ImmediateEventHandler;
use crate::component::memory::MemoryManager;
use crate::component::memory::backend::MemoryBackend;
use crate::component::social::DialogueClient;
use crate::component::social::RelationshipStore;
use crate::config::{CharacterConfig, Config, DeviceConfig};
use crate::infra::api::ReconnectRequest;
use crate::infra::transport::websocket::AgentClient;
use crate::models::Intent;
use crate::runtime::claw::LlmClientContainer;
use crate::soul::reflector::{PersonaInfo, Validator};

use super::builder::AgentBuilder;
use super::{
    DecisionCallback, DecisionWithChainCallback, DecisionWithFeedbackCallback,
    DecisionWithMemoryCallback,
};

// ============================================================================
// Agent
// ============================================================================

/// Agent 运行时
///
/// 管理与服务端的通信和决策循环
pub struct Agent {
    /// 配置
    pub(crate) config: Config,

    /// 客户端
    pub(crate) client: AgentClient,

    /// 决策回调
    pub(crate) decision_callback: DecisionCallback,

    /// 带记忆上下文的决策回调（可选，用于认知引擎集成）
    pub(crate) decision_with_memory_callback: Option<DecisionWithMemoryCallback>,

    /// 带反馈的决策回调（可选，用于验证器集成）
    pub(crate) decision_with_feedback_callback: Option<DecisionWithFeedbackCallback>,

    /// 带 CognitiveChain 的决策回调（人魂直连 WorldState）
    ///
    /// 此回调接收 WorldState，人魂直接输出结构化 Intent。
    /// CognitiveChain 供 soul_cycle_recorder 记录用。
    pub(crate) decision_with_chain_callback: Option<DecisionWithChainCallback>,

    /// 记忆管理器（可选，线程安全）
    pub(crate) memory_manager: Option<Arc<tokio::sync::RwLock<MemoryManager>>>,

    /// 对话客户端（可选）
    pub(crate) dialogue_client: Option<DialogueClient>,

    /// 对话上下文管理器（替代WorkingMemory存储对话）
    pub(crate) dialogue_manager: Option<Arc<tokio::sync::RwLock<DialogueContextManager>>>,

    /// 关系存储（可选）
    pub(crate) relationship_store: Option<RelationshipStore>,

    /// 统一意图审查器（运行时唯一入口）
    pub(crate) validator: Option<std::sync::Arc<dyn Validator>>,

    /// 上一次 ReflectorSoul 驳回原因（跨 tick 传递给 ActorSoul）
    pub(crate) last_rejection_reason: Option<String>,

    /// 注册成功回调（可选，用于更新外部状态如 HTTP API 的 agent_id）
    pub(crate) registration_callback: Option<std::sync::Arc<dyn Fn(Uuid) + Send + Sync>>,

    /// 重连退避计数器（用于逐步降低重试频率）
    pub(crate) reconnect_backoff: u32,

    /// 重连请求接收通道（可选，用于热切换触发重连）
    /// 由 HTTP API 触发重连
    pub(crate) reconnect_rx: Option<broadcast::Receiver<ReconnectRequest>>,

    /// 死亡是否已报告（避免重复日志）
    pub(crate) death_reported: bool,

    /// 自动重生延迟 ticks（从 AgentDied 消息读取，0 = 不自动重生）
    pub(crate) rebirth_delay_ticks: i32,

    /// 死亡时的 tick_id（用于计算重生时机）
    pub(crate) death_tick_id: Option<i64>,

    /// 连续 LLM 失败计数（成功时重置为 0）
    pub(crate) consecutive_llm_failures: u32,

    /// LLM 失败 chaos 模式是否激活
    pub(crate) llm_chaos_active: bool,

    /// ActorSoul LLM Client 容器（支持热重载）
    ///
    /// 与 `ClawDecisionState.llm` 共享同一个 `RwLock`，
    /// 允许热重载时更新 LLM Client，决策回调会自动使用新配置
    pub(crate) actor_llm_container: Option<LlmClientContainer>,

    /// HTTP API 状态（可选，用于更新 current_state 供 Web Panel 查询）
    pub(crate) http_api_state: Option<std::sync::Arc<crate::infra::api::HttpApiState>>,

    /// 设备身份配置（从 device.yaml 加载，或运行时注册生成）
    pub(crate) device_config: Option<DeviceConfig>,

    /// 角色配置（当前活跃角色）
    pub(crate) character_config: Option<CharacterConfig>,

    /// 认知引擎引用（用于注册后更新 agent_name）
    pub(crate) cognitive_engine: Option<std::sync::Arc<crate::soul::actor::CognitiveEngine>>,

    /// 服务器分配的角色名称（注册时由 ServerMessage::Registered.agent_name 填充）
    /// 优先于 character_config.name，解决本地无 character.yaml 时显示"(未创建)"的问题
    pub(crate) server_assigned_name: Option<String>,

    /// 即时事件处理器（处理 ImmediateEvent）
    pub(crate) immediate_handler: Option<Arc<ImmediateEventHandler>>,

    /// Session Triage Engine 后台任务句柄（每游戏日重生）
    pub(crate) session_triage_handle: Option<tokio::task::JoinHandle<Option<String>>>,
    /// 引擎对应的游戏日（摘要提交时需用此值，非当前 game_day）
    pub(crate) session_triage_game_day: Option<i64>,

    /// Server 验证错误反馈通道（Fn callback 写入，主循环消费）
    pub(crate) server_error_feedback: Arc<Mutex<Option<String>>>,

    /// 即时事件缓冲区（Fn callback 写入，主循环消费写入工作记忆）
    pub(crate) immediate_event_buffer: Arc<Mutex<Vec<cyber_jianghu_protocol::WorldEvent>>>,

    /// 连续 idle tick 计数（无有效 intent 或 intent 为 idle 时递增）
    pub(crate) consecutive_idle_count: u32,

    /// 连续 follow 计数（社交死循环防护）
    pub(crate) consecutive_follow_count: u32,

    /// 混沌意图生成器（Sanity 混沌硬逻辑）
    pub(crate) chaos_generator: Option<crate::soul::actor::ChaosGenerator>,
}

impl Agent {
    /// 获取 Agent 构建器
    pub fn builder(config: Config, decision_callback: DecisionCallback) -> AgentBuilder {
        AgentBuilder::new(config, decision_callback)
    }

    /// 创建新的 Agent（简单构造函数）
    ///
    /// 注意：此构造函数不初始化记忆系统。如需启用记忆系统，请使用 `agent::builder()`。
    ///
    /// # Arguments
    /// * `config` - Agent 配置
    /// * `decision_callback` - 决策回调函数
    /// * `reconnect_rx` - 重连请求接收通道（Claw 模式下用于热切换）
    /// * `device_config` - 设备身份配置（可选，首次启动时为 None）
    pub async fn new(
        config: Config,
        decision_callback: DecisionCallback,
        reconnect_rx: Option<broadcast::Receiver<ReconnectRequest>>,
        device_config: Option<DeviceConfig>,
    ) -> Self {
        let client = AgentClient::new(config.server.clone());

        // 设置设备身份（如果已存在）
        if let Some(ref device) = device_config {
            client
                .set_identity(device.device_id, device.auth_token.clone())
                .await;
        }

        Self {
            config,
            client,
            decision_callback,
            decision_with_memory_callback: None,
            decision_with_feedback_callback: None,
            decision_with_chain_callback: None,
            memory_manager: None,
            dialogue_client: None,
            dialogue_manager: None,
            relationship_store: None,
            validator: None,
            last_rejection_reason: None,
            registration_callback: None,
            reconnect_backoff: 0,
            reconnect_rx,
            death_reported: false,
            rebirth_delay_ticks: 0,
            death_tick_id: None,
            consecutive_llm_failures: 0,
            llm_chaos_active: false,
            actor_llm_container: None,
            http_api_state: None,
            device_config,
            character_config: None,
            cognitive_engine: None,
            server_assigned_name: None,
            immediate_handler: None,
            session_triage_handle: None,
            session_triage_game_day: None,
            server_error_feedback: Arc::new(Mutex::new(None)),
            immediate_event_buffer: Arc::new(Mutex::new(Vec::new())),
            consecutive_idle_count: 0,
            consecutive_follow_count: 0,
            chaos_generator: None,
        }
    }

    /// 获取角色名称（如果已创建）
    ///
    /// 优先使用服务器返回的角色名，然后本地配置，最后是 "(未创建)"
    pub(crate) fn character_name(&self) -> &str {
        self.server_assigned_name
            .as_deref()
            .or_else(|| self.character_config.as_ref().map(|c| c.name.as_str()))
            .unwrap_or("(未创建)")
    }

    /// 从 character.yaml 重新加载人设到认知引擎
    ///
    /// 用于注册确认、重连后恢复人设。如果 character.yaml 存在，加载完整人设
    /// 并刷新 PromptCache；否则仅更新名称。
    pub(crate) fn reload_character_persona(&mut self, agent_id: Uuid, name: &str) {
        if let Some(ref engine) = self.cognitive_engine {
            let server_dir = self.config.server_dir(&self.config.server.ws_url);
            let char_yaml = server_dir
                .join("characters")
                .join(agent_id.to_string())
                .join("character.yaml");

            if char_yaml.exists() {
                match crate::config::CharacterConfig::from_file(&char_yaml) {
                    Ok(char_config) => {
                        let prompt = char_config.generate_system_prompt();
                        engine.update_persona(name, &prompt);
                        engine.update_conversation_system_message(&prompt);
                        self.character_config = Some(char_config);
                        info!("已从 character.yaml 重新加载人设并更新认知引擎");
                    }
                    Err(e) => {
                        warn!("加载 character.yaml 失败，仅更新名称: {}", e);
                        engine.update_agent_name(name);
                    }
                }
            } else {
                engine.update_agent_name(name);
            }
        }
    }

    /// 连接服务端
    pub async fn connect(&mut self) -> Result<()> {
        self.client.connect().await?;
        info!("Agent '{}' connected to server", self.character_name());
        Ok(())
    }

    /// 设置对话客户端
    ///
    /// 必须在连接之后调用，因为需要 agent_id
    pub fn set_dialogue_client(&mut self, dialogue_client: DialogueClient) {
        self.dialogue_client = Some(dialogue_client);
        info!("Dialogue client set for agent '{}'", self.character_name());
    }

    /// 设置关系存储
    pub fn set_relationship_store(&mut self, relationship_store: RelationshipStore) {
        self.relationship_store = Some(relationship_store);
        info!(
            "Relationship store set for agent '{}'",
            self.character_name()
        );
    }

    /// 初始化对话上下文管理器
    ///
    /// 从game_rules配置中读取参数，创建DialogueContextManager
    pub fn init_dialogue_manager(&mut self, max_sessions: usize, max_rounds: usize, session_timeout_ticks: i64) {
        use crate::component::dialogue::DialogueContextManager;
        self.dialogue_manager = Some(std::sync::Arc::new(tokio::sync::RwLock::new(
            DialogueContextManager::new(max_sessions, max_rounds, session_timeout_ticks)
        )));
        info!(
            "Dialogue context manager initialized (max_sessions={}, max_rounds={}, timeout={})",
            max_sessions, max_rounds, session_timeout_ticks
        );
    }

    /// 获取 Agent ID
    pub async fn agent_id(&self) -> Option<Uuid> {
        self.client.agent_id().await
    }

    /// 等待 Agent ID 可用（注册后)
    #[allow(dead_code)]
    pub(crate) async fn wait_for_agent_id(&self) -> Result<Uuid> {
        self.client.wait_for_agent_id().await
    }

    /// 获取对话客户端的引用
    pub fn dialogue_client(&self) -> Option<&DialogueClient> {
        self.dialogue_client.as_ref()
    }

    /// 获取关系存储的引用
    pub fn relationship_store(&self) -> Option<&RelationshipStore> {
        self.relationship_store.as_ref()
    }

    /// 获取关系存储的可变引用
    pub fn relationship_store_mut(&mut self) -> Option<&mut RelationshipStore> {
        self.relationship_store.as_mut()
    }

    /// 设置统一意图审查器
    pub fn set_intent_auditor(&mut self, validator: std::sync::Arc<dyn Validator>) {
        self.validator = Some(validator);
        info!("Intent auditor set for agent '{}'", self.character_name());
    }

    /// 设置带反馈的决策回调
    pub fn set_decision_with_feedback_callback(&mut self, callback: DecisionWithFeedbackCallback) {
        self.decision_with_feedback_callback = Some(callback);
        info!(
            "Decision with feedback callback set for agent '{}'",
            self.character_name()
        );
    }

    /// 设置注册成功回调（用于更新外部状态如 HTTP API 的 agent_id）
    pub fn set_registration_callback(
        &mut self,
        callback: std::sync::Arc<dyn Fn(Uuid) + Send + Sync>,
    ) {
        self.registration_callback = Some(callback);
        info!(
            "Registration callback set for agent '{}'",
            self.character_name()
        );
    }

    /// 设置 Server 消息透传回调（用于 OpenClaw 集成）
    ///
    /// 当收到 Server 下行消息时，此回调会被调用，允许将消息
    /// 转发到外部系统（如 OpenClaw）
    pub async fn set_server_msg_callback(
        &self,
        callback: std::sync::Arc<dyn Fn(cyber_jianghu_protocol::ServerMessage) + Send + Sync>,
    ) {
        self.client.set_server_msg_callback(callback).await;
        info!(
            "Server message callback set for agent '{}'",
            self.character_name()
        );
    }

    /// 检查是否启用验证器
    pub fn has_validator(&self) -> bool {
        self.validator.is_some()
    }

    /// 设置即时事件处理器
    pub fn set_immediate_handler(&mut self, handler: Arc<ImmediateEventHandler>) {
        self.immediate_handler = Some(handler);
        info!(
            "Immediate event handler set for agent '{}'",
            self.character_name()
        );
    }

    /// 获取记忆上下文字符串（用于 LLM）
    pub async fn get_memory_context(&self) -> String {
        if let Some(ref manager) = self.memory_manager {
            let manager = manager.read().await;
            manager.build_llm_context().await
        } else {
            String::new()
        }
    }

    /// 检查记忆系统是否已启用
    pub fn has_memory(&self) -> bool {
        self.memory_manager.is_some()
    }

    /// 获取工作记忆中的事件数量
    pub fn working_memory_size(&self) -> usize {
        match &self.memory_manager {
            Some(manager) => {
                let manager = manager.clone();
                // 使用 block_on 在同步方法中调用异步方法
                // 注意：这可能会阻塞线程，但在 MVP 阶段是可以接受的
                // 更好的做法是将此方法改为 async
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let count: usize =
                            manager.read().await.working().count().await.unwrap_or(0);
                        count
                    })
                })
            }
            None => 0,
        }
    }

    /// 获取记忆统计信息
    pub async fn memory_stats(
        &self,
    ) -> Option<crate::component::memory::manager::MemoryManagerStats> {
        if let Some(ref manager) = self.memory_manager {
            let manager = manager.read().await;
            Some(manager.stats().await)
        } else {
            None
        }
    }

    /// 设置记忆管理器
    pub fn set_memory_manager(&mut self, manager: Arc<tokio::sync::RwLock<MemoryManager>>) {
        self.memory_manager = Some(manager);
        info!("Memory manager set for agent '{}'", self.character_name());
    }

    /// 获取记忆管理器的引用
    pub fn memory_manager(&self) -> Option<&Arc<tokio::sync::RwLock<MemoryManager>>> {
        self.memory_manager.as_ref()
    }

    /// 设置设备身份配置
    pub fn set_device_config(&mut self, config: DeviceConfig) {
        self.device_config = Some(config);
    }

    /// 设置角色配置
    pub fn set_character_config(&mut self, config: CharacterConfig) {
        self.character_config = Some(config);
    }

    /// 获取设备身份配置的引用
    pub fn device_config(&self) -> Option<&DeviceConfig> {
        self.device_config.as_ref()
    }

    /// 获取角色配置的引用
    pub fn character_config(&self) -> Option<&CharacterConfig> {
        self.character_config.as_ref()
    }

    /// 处理世界事件并更新记忆
    /// cognitive_engine: 可选，用于叙事合成（人魂处理）
    pub async fn process_events(
        &mut self,
        events: &[crate::models::WorldEvent],
        cognitive_engine: Option<&crate::soul::actor::CognitiveEngine>,
    ) -> Result<()> {
        if let Some(ref mut manager) = self.memory_manager {
            manager
                .write()
                .await
                .process_events(events, cognitive_engine)
                .await?;
        }
        Ok(())
    }

    /// 运行遗忘机制（每 84 tick 调用一次）
    pub async fn run_forgetting(
        &mut self,
        current_tick: i64,
    ) -> Result<crate::component::memory::types::ForgettingReport> {
        if let Some(ref mut manager) = self.memory_manager {
            manager.write().await.run_forgetting(current_tick).await
        } else {
            Ok(crate::component::memory::types::ForgettingReport {
                checked_count: 0,
                archived_count: 0,
                retained_count: 0,
            })
        }
    }

    /// 获取 tick 持续时间
    pub(crate) async fn get_tick_duration(&self) -> Duration {
        self.client
            .game_rules()
            .await
            .map(|r| Duration::from_secs(r.tick_duration_secs))
            .unwrap_or(Duration::from_secs(60))
    }

    /// 检查连续 idle 计数是否达到阈值，触发模型切换
    pub(crate) async fn maybe_rotate_model(&mut self) {
        if let Some(ref container) = self.actor_llm_container {
            let llm = container.read().await;
            // 使用 FallbackLlmClient 的 record_idle 方法
            // 该方法会自动检查阈值并切换到下一个可用模型
            if llm.record_idle() {
                warn!(
                    "LLM 模型已自动切换（连续 idle 达到阈值 {}），consecutive_idle_count 重置为 0",
                    self.config.llm.idle_rotate_threshold
                );
                // 切换后重置计数器，给新模型机会
                self.consecutive_idle_count = 0;
            }
        }
    }

    /// 组装 Pipeline Intent
    ///
    /// 将多个 Intent 组装为 primary + subsequent_intents 的 Pipeline 结构
    pub(crate) fn assemble_pipeline(intents: Vec<Intent>) -> Intent {
        if intents.is_empty() {
            return Intent::new(uuid::Uuid::nil(), 0, "休息", None);
        }
        if intents.len() == 1 {
            return intents.into_iter().next().unwrap();
        }

        let mut iter = intents.into_iter();
        let mut primary = iter.next().unwrap();
        primary.subsequent_intents = iter.collect();
        primary
    }

    /// 提取人设信息
    pub(crate) fn extract_persona(&self) -> PersonaInfo {
        match &self.character_config {
            Some(character) => PersonaInfo {
                name: Some(character.name.clone()),
                gender: character.gender.clone(),
                // 寿命由 Server 控制，此处使用注册年龄作为人设提示
                age: character.age,
                personality: character.personality.clone(),
                values: character.values.clone(),
            },
            None => {
                // 未创建角色时的默认人设
                PersonaInfo {
                    name: None,
                    gender: "未知".to_string(),
                    age: 25,
                    personality: vec![],
                    values: vec![],
                }
            }
        }
    }

    /// 构建世界上下文
    pub(crate) fn build_world_context(&self, world_state: &crate::models::WorldState) -> String {
        super::utils::build_world_context(world_state)
    }

    /// 将天魂（RuleEngine）的技术性驳回转换为人魂可理解的叙事化反馈
    ///
    /// F2 增强：eat/drink/move 驳回已包含可用选项上下文，直接透传给 LLM。
    /// 其他类型驳回仍叙事化处理。
    pub(crate) fn narrativize_rejection(reason: &str) -> String {
        use crate::soul::reflector::rule_engine::engine::{
            ERR_DRINK_INVALID_ITEM, ERR_EAT_INVALID_ITEM, ERR_MOVE_INVALID_TARGET,
        };

        // F2: RuleEngine 增强驳回（含上下文选项）直接透传
        if reason.starts_with(ERR_EAT_INVALID_ITEM)
            || reason.starts_with(ERR_DRINK_INVALID_ITEM)
            || reason.starts_with(ERR_MOVE_INVALID_TARGET)
        {
            return reason.to_string();
        }

        if reason.contains("不在合法列表") {
            return "你想做一件事，但似乎无法如愿。也许该换个行动方式。".to_string();
        }

        // LLM 驳回（人设/世界观）已经是自然语言，直接使用
        reason.to_string()
    }

    /// 获取三魂循环记录器（如果可用）
    /// 获取当前角色的三魂记录器（从注册表按需加载）
    pub(crate) async fn soul_recorder(
        &self,
    ) -> Option<Arc<crate::infra::api::soul_cycle_recorder::SoulCycleRecorder>> {
        let state = self.http_api_state.as_ref()?;
        let agent_id = *state.agent_id.read().await;
        state.soul_recorder_for(agent_id).await
    }
}
