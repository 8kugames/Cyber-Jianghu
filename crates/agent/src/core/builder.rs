// ============================================================================
// Agent Builder
// ============================================================================
//
// 提供流式接口构建 Agent
// ============================================================================

use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, mpsc};
use tracing::info;
use uuid::Uuid;

use crate::ai::dialogue::DialogueClient;
use crate::ai::lifespan::LifespanCalculator;
use crate::ai::llm::LlmClient;
use crate::ai::memory::{MemoryManager, MemoryManagerConfig};
use crate::ai::relationship::RelationshipStore;
use crate::ai::validator::{IntentValidator, Validator};
use crate::config::{Config, ReviewConfig};
use crate::runtime::claw::LlmClientContainer;
use crate::runtime::decision::http::{HttpApiState, ReconnectRequest, review::ReviewStore};
use crate::transport::websocket::AgentClient;
use cyber_jianghu_protocol::WorldBuildingRules;

use super::{
    Agent, DecisionCallback, DecisionWithFeedbackCallback, DecisionWithMemoryCallback,
    ValidatorConfig,
};

/// Agent 构建器
pub struct AgentBuilder {
    config: Config,
    decision_callback: DecisionCallback,
    decision_with_memory_callback: Option<DecisionWithMemoryCallback>,
    decision_with_feedback_callback: Option<DecisionWithFeedbackCallback>,
    enable_memory: bool,
    memory_config: Option<MemoryManagerConfig>,
    llm_client: Option<Arc<dyn LlmClient>>,
    /// LLM Client 容器（支持热重载）
    ///
    /// 与 ClawDecisionState 共享，用于运行时动态切换 LLM Client
    llm_container: Option<LlmClientContainer>,
    dialogue_client: Option<DialogueClient>,
    relationship_store: Option<RelationshipStore>,
    validator: Option<Arc<dyn Validator>>,
    lifespan_calculator: Option<LifespanCalculator>,
    validator_config: ValidatorConfig,
    /// 审查存储（ReflectorSoul 共享）
    review_store: Option<Arc<ReviewStore>>,
    /// 审查配置
    review_config: ReviewConfig,
    /// 重连请求接收通道（Claw 模式）
    reconnect_rx: Option<mpsc::Receiver<crate::runtime::decision::http::ReconnectRequest>>,
    /// 配置重载通知接收通道
    config_reload_rx: Option<broadcast::Receiver<()>>,
    /// HTTP API 状态（可选，用于 Cognitive 模式更新 current_state 供 Web Panel 查询）
    http_api_state: Option<Arc<HttpApiState>>,
}

impl AgentBuilder {
    /// 创建新的构建器
    pub fn new(config: Config, decision_callback: DecisionCallback) -> Self {
        let review_config = config.review.clone().unwrap_or_default();
        Self {
            config,
            decision_callback,
            decision_with_memory_callback: None,
            decision_with_feedback_callback: None,
            enable_memory: true,
            memory_config: None,
            llm_client: None,
            llm_container: None,
            dialogue_client: None,
            relationship_store: None,
            validator: None,
            lifespan_calculator: None,
            validator_config: ValidatorConfig::default(),
            review_store: None,
            review_config,
            reconnect_rx: None,
            config_reload_rx: None,
            http_api_state: None,
        }
    }

    /// 启用/禁用记忆系统
    pub fn enable_memory(mut self, enable: bool) -> Self {
        self.enable_memory = enable;
        self
    }

    /// 设置记忆管理器配置
    pub fn with_memory_config(mut self, config: MemoryManagerConfig) -> Self {
        self.memory_config = Some(config);
        self
    }

    /// 设置带反馈的决策回调
    pub fn with_decision_feedback(mut self, callback: DecisionWithFeedbackCallback) -> Self {
        self.decision_with_feedback_callback = Some(callback);
        self
    }

    /// 设置带记忆上下文的决策回调
    ///
    /// 此回调接收世界状态和记忆上下文，用于认知引擎集成
    pub fn with_decision_memory(mut self, callback: DecisionWithMemoryCallback) -> Self {
        self.decision_with_memory_callback = Some(callback);
        self
    }

    /// 设置对话客户端
    pub fn with_dialogue_client(mut self, client: DialogueClient) -> Self {
        self.dialogue_client = Some(client);
        self
    }

    /// 设置关系存储
    pub fn with_relationship_store(mut self, store: RelationshipStore) -> Self {
        self.relationship_store = Some(store);
        self
    }

    /// 设置验证器
    pub fn with_validator(mut self, validator: Arc<dyn Validator>) -> Self {
        self.validator = Some(validator);
        self
    }

    /// 设置 LLM 客户端（自动创建 IntentValidator）
    pub fn with_llm_client(
        mut self,
        llm_client: Arc<dyn LlmClient>,
        rules: Option<WorldBuildingRules>,
    ) -> Self {
        let rules = rules.unwrap_or_default();
        let validator = Arc::new(IntentValidator::new(rules, llm_client.clone()));
        self.validator = Some(validator);
        self.llm_client = Some(llm_client);
        self
    }

    /// 设置 LLM Client 容器（支持热重载）
    ///
    /// 此方法用于设置共享的 LLM Client 容器，当配置变更时，
    /// 可以通过更新容器内容来实现 LLM Client 的动态切换。
    /// 决策回调会自动使用最新的 LLM Client。
    pub fn with_llm_container(mut self, container: LlmClientContainer) -> Self {
        self.llm_container = Some(container);
        self
    }

    /// 设置寿命计算器
    pub fn with_lifespan_calculator(mut self, calculator: LifespanCalculator) -> Self {
        self.lifespan_calculator = Some(calculator);
        self
    }

    /// 设置验证器配置
    pub fn with_validator_config(mut self, config: ValidatorConfig) -> Self {
        self.validator_config = config;
        self
    }

    /// 设置审查存储（ActorSoul + ReflectorSoul）
    pub fn with_review_store(mut self, store: Arc<ReviewStore>) -> Self {
        self.review_store = Some(store);
        self
    }

    /// 设置重连请求接收通道（Claw 模式热切换）
    pub fn with_reconnect_rx(mut self, rx: mpsc::Receiver<ReconnectRequest>) -> Self {
        self.reconnect_rx = Some(rx);
        self
    }

    /// 设置配置重载通知接收通道
    pub fn with_config_reload_rx(mut self, rx: broadcast::Receiver<()>) -> Self {
        self.config_reload_rx = Some(rx);
        self
    }

    /// 设置 HTTP API 状态（用于 Cognitive 模式更新 current_state 供 Web Panel 查询）
    pub fn with_http_api_state(mut self, state: Arc<HttpApiState>) -> Self {
        self.http_api_state = Some(state);
        self
    }

    /// 构建 Agent
    pub fn build(self) -> Agent {
        let client = AgentClient::new(self.config.server.clone());

        // 设置设备身份（如果已存在）
        if let Some(ref identity) = self.config.identity {
            let device_id = identity.device_id;
            let auth_token = identity.auth_token.clone();
            tokio::task::block_in_place(|| {
                Handle::current().block_on(async {
                    client.set_identity(device_id, auth_token).await;
                });
            });
        }

        // 初始化记忆系统
        let memory_manager = if self.enable_memory {
            let agent_id = Uuid::new_v4();
            let config = self.memory_config.unwrap_or_else(|| MemoryManagerConfig {
                agent_id,
                ..Default::default()
            });

            // 初始化记忆管理器（使用本地 embedder）
            let result = MemoryManager::new(config);

            match result {
                Ok(manager) => {
                    let agent_name = self
                        .config
                        .agent
                        .as_ref()
                        .map(|c| c.name.as_str())
                        .unwrap_or("(未创建)");
                    info!("Memory system initialized for agent '{}'", agent_name);
                    Some(manager)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to initialize memory system: {}. Running without memory.",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        Agent {
            config: self.config,
            client,
            decision_callback: self.decision_callback,
            decision_with_memory_callback: self.decision_with_memory_callback,
            decision_with_feedback_callback: self.decision_with_feedback_callback,
            memory_manager,
            dialogue_client: self.dialogue_client,
            relationship_store: self.relationship_store,
            validator: self.validator,
            lifespan_calculator: self.lifespan_calculator,
            validator_config: self.validator_config,
            registration_callback: None,
            reconnect_backoff: 0,
            reconnect_rx: self.reconnect_rx,
            death_reported: false,
            review_store: self.review_store,
            review_config: self.review_config,
            actor_llm_client: self.llm_client,
            actor_llm_container: self.llm_container,
            config_reload_rx: self.config_reload_rx,
            http_api_state: self.http_api_state,
        }
    }
}
