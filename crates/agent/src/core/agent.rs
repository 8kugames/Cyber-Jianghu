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

use crate::component::immediate::ImmediateEventHandler;
use crate::component::memory::MemoryManager;
use crate::component::memory::backend::MemoryBackend;

use crate::component::memory::types::MemoryEntry;
use crate::component::persona::LifespanCalculator;
use crate::component::social::DialogueClient;
use crate::component::social::RelationshipStore;
use crate::config::{CharacterConfig, Config, DeviceConfig};
use crate::infra::api::ReconnectRequest;
use crate::infra::transport::websocket::AgentClient;
use crate::models::{Intent, WorldState};
use crate::runtime::claw::LlmClientContainer;
use crate::soul::reflector::{PersonaInfo, Validator};

use super::builder::AgentBuilder;
use super::{DecisionCallback, DecisionWithFeedbackCallback, DecisionWithMemoryCallback};

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

    /// 记忆管理器（可选）
    pub(crate) memory_manager: Option<MemoryManager>,

    /// 对话客户端（可选）
    pub(crate) dialogue_client: Option<DialogueClient>,

    /// 关系存储（可选）
    pub(crate) relationship_store: Option<RelationshipStore>,

    /// 意图审查器（ReflectorSoul，可选）
    pub(crate) validator: Option<std::sync::Arc<dyn Validator>>,

    /// 寿命计算器（可选）
    pub(crate) lifespan_calculator: Option<LifespanCalculator>,

    /// 上一次 ReflectorSoul 驳回原因（跨 tick 传递给 ActorSoul）
    pub(crate) last_rejection_reason: Option<String>,

    /// 注册成功回调（可选，用于更新外部状态如 HTTP API 的 agent_id）
    pub(crate) registration_callback: Option<std::sync::Arc<dyn Fn(Uuid) + Send + Sync>>,

    /// 重连退避计数器（用于逐步降低重试频率）
    pub(crate) reconnect_backoff: u32,

    /// 重连请求接收通道（可选，用于热切换触发重连）
    /// Claw 模式下由 HTTP API 触发重连，WebSocket 模式为 None
    pub(crate) reconnect_rx: Option<broadcast::Receiver<ReconnectRequest>>,

    /// 死亡是否已报告（避免重复日志）
    pub(crate) death_reported: bool,

    /// ActorSoul LLM Client 容器（支持热重载）
    ///
    /// 与 `ClawDecisionState.llm` 共享同一个 `RwLock`，
    /// 允许热重载时更新 LLM Client，决策回调会自动使用新配置
    pub(crate) actor_llm_container: Option<LlmClientContainer>,

    /// HTTP API 状态（可选，Cognitive 模式用于更新 current_state 供 Web Panel 查询）
    pub(crate) http_api_state: Option<std::sync::Arc<crate::infra::api::HttpApiState>>,

    /// 设备身份配置（从 device.yaml 加载，或运行时注册生成）
    pub(crate) device_config: Option<DeviceConfig>,

    /// 角色配置（当前活跃角色）
    pub(crate) character_config: Option<CharacterConfig>,

    /// 认知引擎引用（Cognitive 模式，用于注册后更新 agent_name）
    pub(crate) cognitive_engine: Option<std::sync::Arc<crate::soul::actor::CognitiveEngine>>,

    /// 服务器分配的角色名称（注册时由 ServerMessage::Registered.agent_name 填充）
    /// 优先于 character_config.name，解决本地无 character.yaml 时显示"(未创建)"的问题
    pub(crate) server_assigned_name: Option<String>,

    /// 即时事件处理器（处理 ImmediateEvent）
    pub(crate) immediate_handler: Option<Arc<ImmediateEventHandler>>,

    /// Server 验证错误反馈通道（Fn callback 写入，主循环消费）
    pub(crate) server_error_feedback: Arc<Mutex<Option<String>>>,

    /// 即时事件缓冲区（Fn callback 写入，主循环消费写入工作记忆）
    pub(crate) immediate_event_buffer: Arc<Mutex<Vec<cyber_jianghu_protocol::WorldEvent>>>,

    /// RuleEngine 缓存（避免每 tick 重建）
    pub(crate) rule_engine: crate::soul::reflector::rule_engine::RuleEngine,

    /// 连续 idle tick 计数（无有效 intent 或 intent 为 idle 时递增）
    pub(crate) consecutive_idle_count: u32,
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
            memory_manager: None,
            dialogue_client: None,
            relationship_store: None,
            validator: None,
            lifespan_calculator: None,
            last_rejection_reason: None,
            registration_callback: None,
            reconnect_backoff: 0,
            reconnect_rx,
            death_reported: false,
            actor_llm_container: None,
            http_api_state: None,
            device_config,
            character_config: None,
            cognitive_engine: None,
            server_assigned_name: None,
            immediate_handler: None,
            server_error_feedback: Arc::new(Mutex::new(None)),
            immediate_event_buffer: Arc::new(Mutex::new(Vec::new())),
            rule_engine: crate::soul::reflector::rule_engine::RuleEngine::with_default_config(),
            consecutive_idle_count: 0,
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

    /// 设置验证器
    pub fn set_validator(&mut self, validator: std::sync::Arc<dyn Validator>) {
        self.validator = Some(validator);
        info!("Validator set for agent '{}'", self.character_name());
    }

    /// 设置带反馈的决策回调
    pub fn set_decision_with_feedback_callback(&mut self, callback: DecisionWithFeedbackCallback) {
        self.decision_with_feedback_callback = Some(callback);
        info!(
            "Decision with feedback callback set for agent '{}'",
            self.character_name()
        );
    }

    /// 设置寿命计算器
    pub fn set_lifespan_calculator(&mut self, calculator: LifespanCalculator) {
        self.lifespan_calculator = Some(calculator);
        info!(
            "Lifespan calculator set for agent '{}'",
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
                // 使用 block_on 在同步方法中调用异步方法
                // 注意：这可能会阻塞线程，但在 MVP 阶段是可以接受的
                // 更好的做法是将此方法改为 async
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let count: usize = manager.working().count().await.unwrap_or(0);
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
            Some(manager.stats().await)
        } else {
            None
        }
    }

    /// 设置记忆管理器
    pub fn set_memory_manager(&mut self, manager: MemoryManager) {
        self.memory_manager = Some(manager);
        info!("Memory manager set for agent '{}'", self.character_name());
    }

    /// 获取记忆管理器的可变引用
    pub fn memory_manager_mut(&mut self) -> Option<&mut MemoryManager> {
        self.memory_manager.as_mut()
    }

    /// 获取记忆管理器的引用
    pub fn memory_manager(&self) -> Option<&MemoryManager> {
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
    pub async fn process_events(&mut self, events: &[crate::models::WorldEvent]) -> Result<()> {
        if let Some(ref mut manager) = self.memory_manager {
            manager.process_events(events).await?;
        }
        Ok(())
    }

    /// 运行遗忘机制（每 84 tick 调用一次）
    pub async fn run_forgetting(
        &mut self,
        current_tick: i64,
    ) -> Result<crate::component::memory::types::ForgettingReport> {
        if let Some(ref mut manager) = self.memory_manager {
            manager.run_forgetting(current_tick).await
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
        let threshold = self.config.llm.idle_rotate_threshold;
        if threshold == 0 || self.consecutive_idle_count < threshold {
            return;
        }
        let count = self.consecutive_idle_count;
        if let Some(ref container) = self.actor_llm_container {
            let llm = container.read().await;
            if llm.force_rotate_model() {
                warn!(
                    "连续 {} tick idle，已切换到下一个 LLM 模型",
                    count
                );
                // 切换后重置计数器，给新模型机会
                self.consecutive_idle_count = 0;
            }
        }
    }

    /// 提取人设信息
    pub(crate) fn extract_persona(&self) -> PersonaInfo {
        match &self.character_config {
            Some(character) => PersonaInfo {
                gender: character.gender.clone(),
                age: self
                    .lifespan_calculator
                    .as_ref()
                    .map(|c| c.current_age())
                    .unwrap_or(character.age),
                personality: character.personality.clone(),
                values: character.values.clone(),
            },
            None => {
                // 未创建角色时的默认人设
                PersonaInfo {
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
        super::utils::build_world_context(world_state, self.lifespan_calculator.as_ref())
    }

    /// 保存观察者叙事到情景记忆
    pub(crate) async fn save_observer_narrative(
        &mut self,
        tick_id: i64,
        narrative: &str,
    ) -> Result<()> {
        if narrative.is_empty() {
            return Ok(());
        }

        if let Some(ref mut manager) = self.memory_manager {
            let entry = MemoryEntry::new(manager.agent_id(), tick_id, narrative.to_string())
                .with_event_type("observer_narrative".to_string())
                .with_importance(0.7);

            manager.episodic_mut().add(entry).await?;
            info!("Observer narrative saved to episodic memory");
        }
        Ok(())
    }

    /// 验证人设（注册前调用，客户端本地）
    pub async fn validate_persona(&self) -> Result<PersonaValidationResult> {
        let validator = match &self.validator {
            Some(v) => v,
            None => return Ok(PersonaValidationResult::Skipped),
        };

        let persona = self.extract_persona();

        match validator.validate_persona(&persona).await? {
            crate::soul::reflector::ValidationResult::Approved { .. } => {
                Ok(PersonaValidationResult::Approved)
            }
            crate::soul::reflector::ValidationResult::Rejected {
                reason,
                rejection_type,
            } => Ok(PersonaValidationResult::NeedsRevision {
                reason,
                rejection_type,
            }),
        }
    }

    /// RuleEngine 规则校验（确定性，不经过 LLM）
    ///
    /// 使用默认规则集（eat/drink item_id 有效性、move 目标可达性等）。
    /// 从 WorldState 提取背包物品 ID 和可达地点 ID 供规则匹配。
    async fn validate_with_rule_engine(
        &self,
        intent: &Intent,
        world_state: &WorldState,
    ) -> Result<(), String> {
        use crate::soul::reflector::rule_engine::{
            RuleValidationContext,
            types::extract_ids_from_world_state,
        };

        let (available_item_ids, reachable_node_ids) =
            extract_ids_from_world_state(world_state);

        let context = RuleValidationContext {
            intent: intent.clone(),
            persona_info: self.extract_persona(),
            world_context: String::new(),
            tick_id: world_state.tick_id,
            history_intents: vec![],
            attributes: std::collections::HashMap::new(),
            available_item_ids,
            reachable_node_ids,
        };

        match self.rule_engine.validate_context(&context).await {
            Ok(crate::soul::reflector::ValidationResult::Approved { .. }) => Ok(()),
            Ok(crate::soul::reflector::ValidationResult::Rejected { reason, .. }) => Err(reason),
            Err(e) => {
                // 规则引擎出错时放行（fail-open）
                tracing::warn!("RuleEngine error, bypassing: {}", e);
                Ok(())
            }
        }
    }

    /// 确定性 action_type 校验（不经过 LLM）
    ///
    /// 从本地 actions.json 加载合法 action 列表，检查 intent 的 action_type 是否在列。
    /// idle 动作始终放行。
    /// actions.json 不存在时放行（无数据不做拦截）。
    fn validate_action_type(&self, intent: &Intent) -> Result<(), String> {
        // idle 始终合法
        if intent.action_type.as_str() == "idle" {
            return Ok(());
        }

        let actions = crate::infra::api::cognitive_context::load_available_actions_from_file();
        if actions.is_empty() {
            // 无数据不做拦截
            return Ok(());
        }

        let valid_names: Vec<&str> = actions.iter().map(|a| a.action.as_str()).collect();
        if valid_names.contains(&intent.action_type.as_str()) {
            return Ok(());
        }

        // 找最接近的合法 action（简单前缀匹配 + 包含匹配）
        let action_lower = intent.action_type.as_str().to_lowercase();
        let suggestion = valid_names
            .iter()
            .find(|name| {
                let name_lower = name.to_lowercase();
                name_lower.contains(&action_lower) || action_lower.contains(&name_lower)
            })
            .unwrap_or(&"idle");

        Err(format!(
            "action '{}' 不在合法列表中，合法值: [{}]，最接近: '{}'",
            intent.action_type,
            valid_names.join(", "),
            suggestion,
        ))
    }

    /// ReflectorSoul 同步审查 Intent
    ///
    /// 三层审查，规则型在 LLM 之前：
    /// 1. action_type 确定性校验：是否在合法动作列表中
    /// 2. RuleEngine 规则校验：eat/drink item_id 有效性、move 目标可达性等
    /// 3. LLM 审查：人设/世界观合规
    ///
    /// 审查通过返回 Approved，审查拒绝返回 Rejected(reason)。
    /// 调用方根据返回值决定是否重试 ActorSoul。
    pub async fn validate_with_reflector(
        &mut self,
        intent: Intent,
        world_state: &WorldState,
    ) -> Result<ReflectorResult> {
        // 第一层：action_type 确定性校验（不经过 LLM）
        if let Err(e) = self.validate_action_type(&intent) {
            warn!("Action type validation failed: {}", e);
            self.last_rejection_reason = Some(e.clone());
            return Ok(ReflectorResult::Rejected(e));
        }

        // 第二层：RuleEngine 规则校验（确定性，不经过 LLM）
        if let Err(e) = self.validate_with_rule_engine(&intent, world_state).await {
            warn!("Rule engine validation failed: {}", e);
            self.last_rejection_reason = Some(e.clone());
            return Ok(ReflectorResult::Rejected(e));
        }

        // 第三层：LLM 审查（人设/世界观）
        let validator = match &self.validator {
            Some(v) => v,
            None => return Ok(ReflectorResult::Approved(intent)),
        };

        let request = crate::soul::reflector::ValidationRequest {
            intent: intent.clone(),
            persona: self.extract_persona(),
            world_context: self.build_world_context(world_state),
            world_state: Some(world_state.clone()),
        };

        // LLM 错误时 fail-open（自动通过）
        let validation_result = match validator.validate(request).await {
            Ok(result) => result,
            Err(e) => {
                warn!("ReflectorSoul validation error, auto-approving: {}", e);
                return Ok(ReflectorResult::Approved(intent));
            }
        };

        match validation_result {
            crate::soul::reflector::ValidationResult::Approved { reason, narrative } => {
                info!("ReflectorSoul approved: {:?}", reason);
                self.save_observer_narrative(world_state.tick_id, &narrative)
                    .await?;
                Ok(ReflectorResult::Approved(intent))
            }
            crate::soul::reflector::ValidationResult::Rejected {
                reason,
                rejection_type,
            } => {
                warn!("ReflectorSoul rejected: {} [{:?}]", reason, rejection_type);
                self.last_rejection_reason = Some(reason.clone());
                Ok(ReflectorResult::Rejected(reason))
            }
        }
    }
}

/// ReflectorSoul 审查结果
pub enum ReflectorResult {
    /// 审查通过，携带修正后的 Intent
    Approved(Intent),
    /// 审查拒绝，携带原因（调用方可据此重试 ActorSoul）
    Rejected(String),
}

/// 人设验证结果
#[derive(Debug)]
pub enum PersonaValidationResult {
    /// 验证通过
    Approved,
    /// 需要修改
    NeedsRevision {
        reason: String,
        rejection_type: crate::soul::reflector::RejectionType,
    },
    /// 跳过验证（无验证器）
    Skipped,
}
