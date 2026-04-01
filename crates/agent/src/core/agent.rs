// ============================================================================
// Agent 核心
// ============================================================================
//
// Agent 结构定义和基本方法
// ============================================================================

use anyhow::Result;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};
use uuid::Uuid;

use crate::component::llm::LlmClient;
use crate::component::memory::MemoryManager;
use crate::component::memory::backend::MemoryBackend;
use crate::component::memory::tools::{MemoryToolDefinition, MemoryToolResult};
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
    pub(crate) reconnect_rx: Option<mpsc::Receiver<ReconnectRequest>>,

    /// 死亡是否已报告（避免重复日志）
    pub(crate) death_reported: bool,

    /// ActorSoul 当前 LLM Client
    pub(crate) actor_llm_client: Option<std::sync::Arc<dyn LlmClient>>,

    /// ActorSoul LLM Client 容器（支持热重载）
    ///
    /// 与 `ClawDecisionState.llm` 共享同一个 `RwLock`，
    /// 允许热重载时更新 LLM Client，决策回调会自动使用新配置
    pub(crate) actor_llm_container: Option<LlmClientContainer>,

    /// 配置重载通知接收通道
    pub(crate) config_reload_rx: Option<broadcast::Receiver<()>>,

    /// HTTP API 状态（可选，Cognitive 模式用于更新 current_state 供 Web Panel 查询）
    pub(crate) http_api_state: Option<std::sync::Arc<crate::infra::api::HttpApiState>>,

    /// 设备身份配置（从 device.yaml 加载，或运行时注册生成）
    pub(crate) device_config: Option<DeviceConfig>,

    /// 角色配置（当前活跃角色）
    pub(crate) character_config: Option<CharacterConfig>,
}

impl Agent {
    /// 获取 Agent 构建器
    pub fn builder(config: Config, decision_callback: DecisionCallback) -> AgentBuilder {
        AgentBuilder::new(config, decision_callback)
    }

    /// 创建新的 Agent（简单构造函数）
    ///
    /// 注意：此构造函数不初始化记忆系统。如需启用记忆系统，请使用 `Agent::builder()`。
    ///
    /// # Arguments
    /// * `config` - Agent 配置
    /// * `decision_callback` - 决策回调函数
    /// * `reconnect_rx` - 重连请求接收通道（Claw 模式下用于热切换）
    /// * `device_config` - 设备身份配置（可选，首次启动时为 None）
    pub async fn new(
        config: Config,
        decision_callback: DecisionCallback,
        reconnect_rx: Option<mpsc::Receiver<ReconnectRequest>>,
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
            actor_llm_client: None,
            actor_llm_container: None,
            config_reload_rx: None,
            http_api_state: None,
            device_config,
            character_config: None,
        }
    }

    /// 获取角色名称（如果已创建）
    pub(crate) fn character_name(&self) -> &str {
        self.character_config
            .as_ref()
            .map(|c| c.name.as_str())
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

    /// 获取所有记忆工具定义（供 LLM function calling）
    pub fn get_memory_tools() -> Vec<MemoryToolDefinition> {
        super::tools::get_memory_tools()
    }

    /// 执行工具调用
    #[allow(dead_code)]
    async fn execute_tool_call(
        &mut self,
        world_state: &crate::models::WorldState,
        tool_name: &str,
        arguments: &str,
    ) -> MemoryToolResult {
        super::tools::execute_tool_call(&mut self.memory_manager, world_state, tool_name, arguments)
            .await
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

    /// ReflectorSoul 同步审查 Intent
    ///
    /// 单次 LLM 调用，无 retry 循环。
    /// 审查通过返回原始 Intent，审查拒绝返回 idle Intent。
    /// LLM 错误时 fail-open（自动通过）。
    pub async fn validate_with_reflector(&mut self, intent: Intent, world_state: &WorldState) -> Result<Intent> {
        let validator = match &self.validator {
            Some(v) => v,
            None => return Ok(intent),
        };

        let request = crate::soul::reflector::ValidationRequest {
            intent: intent.clone(),
            persona: self.extract_persona(),
            world_context: self.build_world_context(world_state),
        };

        // 验证意图（验证失败时降级为通过，不中断 agent）
        let validation_result = match validator.validate(request).await {
            Ok(result) => result,
            Err(e) => {
                warn!("ReflectorSoul validation error, auto-approving: {}", e);
                return Ok(intent);
            }
        };

        match validation_result {
            crate::soul::reflector::ValidationResult::Approved { reason, narrative } => {
                info!("ReflectorSoul approved: {:?}", reason);
                self.save_observer_narrative(world_state.tick_id, &narrative).await?;
                self.last_rejection_reason = None;
                Ok(intent)
            }
            crate::soul::reflector::ValidationResult::Rejected { reason, rejection_type } => {
                warn!("ReflectorSoul rejected: {} [{:?}]", reason, rejection_type);
                self.last_rejection_reason = Some(reason.clone());
                let agent_id = self.client.agent_id().await.unwrap_or_default();
                Ok(Intent::new(agent_id, world_state.tick_id, "idle", None)
                    .with_thought(format!("被反思之魂驳回: {}", reason)))
            }
        }
    }
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
