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

use crate::ai::dialogue::DialogueClient;
use crate::ai::lifespan::LifespanCalculator;
use crate::ai::llm::LlmClient;
use crate::ai::memory::MemoryManager;
use crate::ai::memory::backend::MemoryBackend;
use crate::ai::memory::tools::{MemoryToolDefinition, MemoryToolResult};
use crate::ai::memory::types::MemoryEntry;
use crate::ai::relationship::RelationshipStore;
use crate::ai::validator::{PersonaInfo, Validator};
use crate::config::{Config, ReviewConfig};
use crate::core::cognitive::MultiStageCognitiveEngine;
use crate::models::{Intent, WorldState};
use crate::runtime::claw::LlmClientContainer;
use crate::runtime::decision::http::{ReconnectRequest, review::ReviewStore};
use crate::transport::websocket::AgentClient;

use super::builder::AgentBuilder;
use super::{DecisionCallback, DecisionWithFeedbackCallback, DecisionWithMemoryCallback};

// ============================================================================
// 验证器配置
// ============================================================================

/// 验证器配置
#[derive(Debug, Clone)]
pub struct ValidatorConfig {
    /// 最大重试次数
    pub max_retry_attempts: u32,

    /// 最小重试时间（秒）
    pub min_retry_time_secs: u64,

    /// 连续驳回后强制 idle 的阈值
    pub consecutive_rejection_threshold: u32,
}

impl Default for ValidatorConfig {
    fn default() -> Self {
        Self {
            max_retry_attempts: 5,
            min_retry_time_secs: 10,
            consecutive_rejection_threshold: 3,
        }
    }
}

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

    /// 意图验证器（可选）
    pub(crate) validator: Option<std::sync::Arc<dyn Validator>>,

    /// 寿命计算器（可选）
    pub(crate) lifespan_calculator: Option<LifespanCalculator>,

    /// 验证器配置
    pub(crate) validator_config: ValidatorConfig,

    /// 注册成功回调（可选，用于更新外部状态如 HTTP API 的 agent_id）
    pub(crate) registration_callback: Option<std::sync::Arc<dyn Fn(Uuid) + Send + Sync>>,

    /// 重连退避计数器（用于逐步降低重试频率）
    pub(crate) reconnect_backoff: u32,

    /// 重连请求接收通道（可选，用于热切换触发重连）
    /// Claw 模式下由 HTTP API 触发重连，WebSocket 模式为 None
    pub(crate) reconnect_rx: Option<mpsc::Receiver<ReconnectRequest>>,

    /// 死亡是否已报告（避免重复日志）
    pub(crate) death_reported: bool,

    /// 审查存储（ReflectorSoul 共享，用于 ActorSoul 提交审查）
    pub(crate) review_store: Option<std::sync::Arc<ReviewStore>>,

    /// 审查配置
    pub(crate) review_config: ReviewConfig,

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
    pub(crate) http_api_state: Option<std::sync::Arc<crate::runtime::decision::http::HttpApiState>>,

    /// 认知引擎引用（可选，用于 config reload 时更新人设）
    pub(crate) cognitive_engine: Option<std::sync::Arc<MultiStageCognitiveEngine>>,
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
    pub async fn new(
        config: Config,
        decision_callback: DecisionCallback,
        reconnect_rx: Option<mpsc::Receiver<ReconnectRequest>>,
    ) -> Self {
        let client = AgentClient::new(config.server.clone());
        let review_config = config.review.clone().unwrap_or_default();

        // 设置设备身份（如果已存在）
        if let Some(ref identity) = config.identity {
            client
                .set_identity(identity.device_id, identity.auth_token.clone())
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
            validator_config: ValidatorConfig::default(),
            registration_callback: None,
            reconnect_backoff: 0,
            reconnect_rx,
            death_reported: false,
            review_store: None,
            review_config,
            actor_llm_client: None,
            actor_llm_container: None,
            config_reload_rx: None,
            http_api_state: None,
            cognitive_engine: None,
        }
    }

    /// 获取角色名称（如果已创建）
    pub(crate) fn character_name(&self) -> &str {
        self.config
            .agent
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

    /// 设置验证器配置
    pub fn set_validator_config(&mut self, config: ValidatorConfig) {
        self.validator_config = config;
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
    pub async fn memory_stats(&self) -> Option<crate::ai::memory::manager::MemoryManagerStats> {
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
    ) -> Result<crate::ai::memory::types::ForgettingReport> {
        if let Some(ref mut manager) = self.memory_manager {
            manager.run_forgetting(current_tick).await
        } else {
            Ok(crate::ai::memory::types::ForgettingReport {
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
        match &self.config.agent {
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
            crate::ai::validator::ValidationResult::Approved { .. } => {
                Ok(PersonaValidationResult::Approved)
            }
            crate::ai::validator::ValidationResult::Rejected {
                reason,
                rejection_type,
            } => Ok(PersonaValidationResult::NeedsRevision {
                reason,
                rejection_type,
            }),
        }
    }

    /// 带验证的决策循环
    ///
    /// `pipeline_deadline`: 整个 pipeline（认知+验证+审查）的绝对截止时间
    pub async fn decide_with_validation(
        &mut self,
        world_state: &WorldState,
        pipeline_deadline: Option<std::time::Instant>,
    ) -> Result<Intent> {
        use std::time::Instant;
        use tracing::warn;

        let tick_start = Instant::now();
        let tick_duration = self.get_tick_duration().await;
        let min_retry_time =
            std::time::Duration::from_secs(self.validator_config.min_retry_time_secs);
        let max_attempts = self.validator_config.max_retry_attempts;

        let mut attempt = 0;
        let mut consecutive_rejections = 0;
        let mut last_rejection_reason: Option<String> = None;

        loop {
            attempt += 1;

            // 检查剩余时间（取 pipeline deadline 和 tick duration 的更紧约束）
            let elapsed = tick_start.elapsed();
            let remaining_tick = tick_duration.saturating_sub(elapsed);
            let remaining = pipeline_deadline
                .and_then(|dl| dl.checked_duration_since(Instant::now()))
                .map(|rd| rd.min(remaining_tick))
                .unwrap_or(remaining_tick);

            if remaining < min_retry_time {
                warn!("Tick time exhausted, forcing idle");
                let agent_id = self.client.agent_id().await.unwrap_or_default();
                return Ok(Intent::new(agent_id, world_state.tick_id, "idle", None));
            }

            if attempt > max_attempts {
                warn!("Max validation attempts reached, forcing idle");
                let agent_id = self.client.agent_id().await.unwrap_or_default();
                return Ok(Intent::new(agent_id, world_state.tick_id, "idle", None));
            }

            // 调用决策回调（可能包含驳回反馈）
            let intent = if let Some(ref reason) = last_rejection_reason {
                if let Some(ref callback) = self.decision_with_feedback_callback {
                    callback(world_state, Some(reason.as_str())).await
                } else {
                    // 如果没有带反馈的回调，记录警告并使用普通回调
                    warn!(
                        "Validation feedback available but no feedback callback set: {}",
                        reason
                    );
                    (self.decision_callback)(world_state).await
                }
            } else {
                (self.decision_callback)(world_state).await
            };

            // 如果没有验证器，直接返回意图
            let validator = match &self.validator {
                Some(v) => v,
                None => return Ok(intent),
            };

            // 构建验证请求（世界观规则由验证器内部维护）
            let request = crate::ai::validator::ValidationRequest {
                intent: intent.clone(),
                persona: self.extract_persona(),
                world_context: self.build_world_context(world_state),
            };

            // 验证意图
            match validator.validate(request).await? {
                crate::ai::validator::ValidationResult::Approved { reason, narrative } => {
                    info!("Intent approved (attempt {}): {:?}", attempt, reason);

                    // 保存叙事摘要到情景记忆
                    self.save_observer_narrative(world_state.tick_id, &narrative)
                        .await?;

                    return Ok(intent);
                }
                crate::ai::validator::ValidationResult::Rejected {
                    reason,
                    rejection_type,
                } => {
                    consecutive_rejections += 1;
                    warn!(
                        "Intent rejected (attempt {}, consecutive: {}): {} [{:?}]",
                        attempt, consecutive_rejections, reason, rejection_type
                    );

                    // 连续驳回次数过多，强制 idle
                    if consecutive_rejections
                        >= self.validator_config.consecutive_rejection_threshold
                    {
                        warn!("Too many consecutive rejections, forcing idle");
                        let agent_id = self.client.agent_id().await.unwrap_or_default();
                        return Ok(Intent::new(agent_id, world_state.tick_id, "idle", None));
                    }

                    // 记录驳回原因，用于下一次决策
                    last_rejection_reason = Some(reason);
                }
            }
        }
    }

    /// 提交 Intent 给 ReflectorSoul 审查
    ///
    /// ActorSoul 生成 Intent 后，提交给 ReflectorSoul 进行审查
    /// 等待审查结果后返回最终 Intent（通过、拒绝或超时降级）
    ///
    /// `pipeline_deadline`: 整个 pipeline 的绝对截止时间，审查超时取 min(配置超时, 剩余时间)
    pub async fn submit_for_review(
        &self,
        intent: Intent,
        world_state: &WorldState,
        review_store: &std::sync::Arc<ReviewStore>,
        pipeline_deadline: Option<std::time::Instant>,
    ) -> Result<Intent> {
        use cyber_jianghu_protocol::PersonaSummary;
        use std::time::Instant;

        // --- 生存底线检查 ---
        // 当 hunger/thirst 低于阈值时，自动批准生存相关行动，绕过 ReflectorSoul 审查
        // 防止"孤僻多疑"等人设因拒绝进食/饮水而陷入死锁
        // survival_actions 列表来自 actions.yaml 中 tags: [survival] 的动作
        const SURVIVAL_THRESHOLD: i32 = 30;

        let survival_actions: &[String] = &self
            .config
            .game_rules
            .as_ref()
            .map(|gr| gr.survival_actions.clone())
            .unwrap_or_default();

        let is_survival_action = survival_actions
            .iter()
            .any(|a| a == intent.action_type.as_str());

        if is_survival_action {
            let hunger = world_state.self_state.hunger();
            let thirst = world_state.self_state.thirst();

            if hunger < SURVIVAL_THRESHOLD || thirst < SURVIVAL_THRESHOLD {
                info!(
                    "[ActorSoul] Survival mode bypass: hunger={}, thirst={}, action={}",
                    hunger, thirst, intent.action_type
                );
                return Ok(intent);
            }
        }

        let persona = self.extract_persona();
        let character_name = self
            .config
            .agent
            .as_ref()
            .map(|c| c.name.as_str())
            .unwrap_or("(未创建)");

        // 构建 PersonaSummary（ReflectorSoul 使用）
        let persona_summary = PersonaSummary {
            name: character_name.to_string(),
            gender: persona.gender.clone(),
            age: persona.age,
            personality: persona.personality.clone(),
            values: persona.values.clone(),
        };

        // 添加到待审查队列
        let intent_id = review_store
            .add_pending(
                intent.clone(),
                self.client.agent_id().await.unwrap_or_default(),
                persona_summary,
                self.build_world_context(world_state),
            )
            .await;

        info!(
            "[ActorSoul] Submitted intent {} for ReflectorSoul review",
            intent_id
        );

        // 等待 ReflectorSoul 审查结果（带超时）
        // 优先使用 pipeline_deadline 的剩余时间，fallback 到配置超时
        let configured_timeout = std::time::Duration::from_secs(self.review_config.timeout_seconds);
        let deadline = match pipeline_deadline {
            Some(dl) => {
                let remaining = dl.saturating_duration_since(Instant::now());
                // 剩余时间不足 5s，直接跳过审查
                if remaining < std::time::Duration::from_secs(5) {
                    warn!(
                        "[ActorSoul] Pipeline deadline exhausted, skipping review for intent {}",
                        intent_id
                    );
                    return Ok(intent);
                }
                // 取 min(配置超时, 剩余时间)
                Instant::now() + remaining.min(configured_timeout)
            }
            None => Instant::now() + configured_timeout,
        };

        loop {
            // 检查超时
            if Instant::now() > deadline {
                warn!(
                    "[ActorSoul] Review timeout for intent {}, using original intent",
                    intent_id
                );
                return Ok(intent);
            }

            // 检查审查状态
            if let Some(result) = review_store.get_status(intent_id).await {
                use crate::runtime::decision::http::review::ReviewStatus;
                match result.status {
                    ReviewStatus::Approved => {
                        info!(
                            "[ActorSoul] Intent approved by ReflectorSoul: {:?}",
                            result.reason
                        );
                        let mut final_intent = intent;
                        if let Some(reason) = &result.reason {
                            final_intent = final_intent.with_observer_thought(reason.clone());
                        }
                        if let Some(narrative) = &result.narrative {
                            final_intent = final_intent.with_narrative(narrative.clone());
                        }
                        return Ok(final_intent);
                    }
                    ReviewStatus::Rejected => {
                        warn!(
                            "[ActorSoul] Intent rejected by ReflectorSoul: {:?}",
                            result.reason
                        );
                        let mut idle_intent = Intent::new(
                            self.client.agent_id().await.unwrap_or_default(),
                            world_state.tick_id,
                            "idle",
                            None,
                        );
                        idle_intent = idle_intent.with_thought(format!(
                            "被反思之魂驳回: {}",
                            result.reason.clone().unwrap_or_default()
                        ));
                        if let Some(reason) = &result.reason {
                            idle_intent = idle_intent.with_observer_thought(reason.clone());
                        }
                        if let Some(narrative) = &result.narrative {
                            idle_intent = idle_intent.with_narrative(narrative.clone());
                        }
                        return Ok(idle_intent);
                    }
                    ReviewStatus::TimeoutApproved => {
                        info!(
                            "[ActorSoul] Review timeout approved for intent {}",
                            intent_id
                        );
                        let mut final_intent = intent;
                        if let Some(reason) = &result.reason {
                            final_intent = final_intent.with_observer_thought(reason.clone());
                        }
                        if let Some(narrative) = &result.narrative {
                            final_intent = final_intent.with_narrative(narrative.clone());
                        }
                        return Ok(final_intent);
                    }
                    ReviewStatus::Pending => {
                        // 继续等待
                    }
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
        rejection_type: crate::ai::validator::RejectionType,
    },
    /// 跳过验证（无验证器）
    Skipped,
}
