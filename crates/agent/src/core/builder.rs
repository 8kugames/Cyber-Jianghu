// ============================================================================
// Agent Builder
// ============================================================================
//
// 提供流式接口构建 Agent
// ============================================================================

use std::path::PathBuf;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;

use crate::component::dialogue::DialogueContextManager;
use crate::component::immediate::{EventStore, ImmediateEventHandler};
use crate::component::llm::LlmClient;
use crate::component::memory::{MemoryManager, MemoryManagerConfig};
use crate::component::persona::{EventTraitMapper, PersonaStore, ThreadSafePersona};
use crate::component::social::DialogueClient;
use crate::component::social::RelationshipStore;
use crate::config::{CharacterConfig, Config, DeviceConfig};
use crate::infra::api::{HttpApiState, ReconnectRequest};
use crate::infra::transport::websocket::AgentClient;
use crate::runtime::claw::LlmClientContainer;
use crate::soul::reflector::{ReflectorSoul, Validator};
use cyber_jianghu_protocol::WorldBuildingRules;

use super::{
    Agent, DecisionCallback, DecisionWithChainCallback, DecisionWithFeedbackCallback,
    DecisionWithMemoryCallback,
};

/// Agent 构建器
pub struct AgentBuilder {
    config: Config,
    decision_callback: DecisionCallback,
    decision_with_memory_callback: Option<DecisionWithMemoryCallback>,
    decision_with_feedback_callback: Option<DecisionWithFeedbackCallback>,
    decision_with_chain_callback: Option<DecisionWithChainCallback>,
    enable_memory: bool,
    memory_config: Option<MemoryManagerConfig>,
    llm_client: Option<Arc<dyn LlmClient>>,
    /// LLM Client 容器（支持热重载）
    ///
    /// 与 ClawDecisionState 共享，用于运行时动态切换 LLM Client
    llm_container: Option<LlmClientContainer>,
    dialogue_client: Option<DialogueClient>,
    dialogue_manager: Option<Arc<tokio::sync::RwLock<DialogueContextManager>>>,
    relationship_store: Option<RelationshipStore>,
    validator: Option<Arc<dyn Validator>>,
    /// 重连请求接收通道
    reconnect_rx: Option<broadcast::Receiver<crate::infra::api::ReconnectRequest>>,
    /// HTTP API 状态（可选，用于更新 current_state 供 Web Panel 查询）
    http_api_state: Option<Arc<HttpApiState>>,
    /// 设备身份配置（可选，从 device.yaml 加载）
    device_config: Option<DeviceConfig>,
    /// 角色配置（可选，当前活跃角色）
    character_config: Option<CharacterConfig>,
    /// Cognitive Engine 引用（用于注册后更新 agent_name）
    cognitive_engine: Option<std::sync::Arc<crate::soul::actor::CognitiveEngine>>,
    /// 数据目录
    data_dir: PathBuf,
    /// 即时事件处理器
    immediate_handler: Option<std::sync::Arc<ImmediateEventHandler>>,
    /// 混沌意图生成器（Sanity 混沌硬逻辑）
    chaos_generator: Option<crate::soul::actor::ChaosGenerator>,
    /// WorldState 本地落存（含 prev/curr，供 Delta Engine 使用）
    world_state_store: Option<Arc<crate::component::state_store::WorldStateStore>>,
    /// Delta Engine（增量检测，零 token）
    delta_engine: Option<crate::component::delta_engine::DeltaEngine>,
    /// Attention Controller（规则过滤 + 轻量 LLM 排序）
    attention_controller: Option<crate::component::attention::AttentionController>,
    /// 人设（CU-5：默认初始人设；调用方可通过 `with_persona` 注入）
    persona: Option<ThreadSafePersona>,
    /// 事件→特质映射器（默认 `EventTraitMapper::new()`）
    event_trait_mapper: Option<std::sync::Arc<std::sync::RwLock<EventTraitMapper>>>,
    persona_store: Option<std::sync::Arc<PersonaStore>>,
    /// 情绪配置
    emotion_config: Option<crate::component::emotion::config::EmotionConfig>,
}

impl AgentBuilder {
    /// 创建新的构建器
    pub fn new(config: Config, decision_callback: DecisionCallback) -> Self {
        Self {
            config,
            decision_callback,
            decision_with_memory_callback: None,
            decision_with_feedback_callback: None,
            decision_with_chain_callback: None,
            enable_memory: true,
            memory_config: None,
            llm_client: None,
            llm_container: None,
            dialogue_client: None,
            dialogue_manager: None,
            relationship_store: None,
            validator: None,
            reconnect_rx: None,
            http_api_state: None,
            device_config: None,
            character_config: None,
            cognitive_engine: None,
            data_dir: PathBuf::from("."),
            immediate_handler: None,
            chaos_generator: None,
            world_state_store: None,
            delta_engine: None,
            attention_controller: None,
            persona: None,
            event_trait_mapper: None,
            persona_store: None,
            emotion_config: None,
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

    /// 设置带 CognitiveChain 的决策回调
    ///
    /// 此回调返回 (Intent, Option<CognitiveChain>) 元组，
    /// 用于三魂架构中传递 WorldState 给人魂，人魂直连输出结构化 Intent。
    ///
    /// 当设置了此回调时，将优先于 `with_decision_memory` 和 `with_decision_feedback` 使用。
    pub fn with_decision_chain(mut self, callback: DecisionWithChainCallback) -> Self {
        self.decision_with_chain_callback = Some(callback);
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

    /// 设置统一意图审查器
    pub fn with_intent_auditor(mut self, validator: Arc<dyn Validator>) -> Self {
        self.validator = Some(validator);
        self
    }

    /// 设置 LLM 客户端（自动创建 ReflectorSoul，共享 LlmClientContainer 支持热重载）
    pub fn with_llm_client(
        mut self,
        llm_client: Arc<dyn LlmClient>,
        rules: Option<WorldBuildingRules>,
    ) -> Self {
        let rules = rules.expect("WorldBuildingRules 必须由 server 下发，配置缺失请联系管理员");
        // 复用已有 container 或创建新的，确保 ActorSoul 和 ReflectorSoul 共享
        let container = self
            .llm_container
            .clone()
            .unwrap_or_else(|| Arc::new(RwLock::new(llm_client.clone())));
        let validator = Arc::new(ReflectorSoul::new(rules, container.clone()));

        self.validator = Some(validator);
        self.llm_client = Some(llm_client);
        self.llm_container = Some(container);
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

    /// 设置重连请求接收通道
    pub fn with_reconnect_rx(mut self, rx: broadcast::Receiver<ReconnectRequest>) -> Self {
        self.reconnect_rx = Some(rx);
        self
    }

    /// 设置 HTTP API 状态（用于更新 current_state 供 Web Panel 查询）
    pub fn with_http_api_state(mut self, state: Arc<HttpApiState>) -> Self {
        self.http_api_state = Some(state);
        self
    }

    /// 设置设备身份配置
    pub fn device_config(mut self, config: DeviceConfig) -> Self {
        self.device_config = Some(config);
        self
    }

    /// 设置角色配置
    pub fn character_config(mut self, config: CharacterConfig) -> Self {
        self.character_config = Some(config);
        self
    }

    /// 设置 Cognitive Engine 引用（用于注册后更新 agent_name）
    pub fn cognitive_engine(
        mut self,
        engine: std::sync::Arc<crate::soul::actor::CognitiveEngine>,
    ) -> Self {
        self.cognitive_engine = Some(engine);
        self
    }

    /// 设置混沌意图生成器（Sanity 混沌硬逻辑）
    pub fn with_chaos_generator(mut self, generator: crate::soul::actor::ChaosGenerator) -> Self {
        self.chaos_generator = Some(generator);
        self
    }

    /// 设置 WorldStateStore（Agent 侧 WorldState 本地落存，供 Delta Engine 使用）
    pub fn with_world_state_store(
        mut self,
        store: Arc<crate::component::state_store::WorldStateStore>,
    ) -> Self {
        self.world_state_store = Some(store);
        self
    }

    /// 设置 Delta Engine（增量检测，零 token）
    pub fn with_delta_engine(
        mut self,
        engine: crate::component::delta_engine::DeltaEngine,
    ) -> Self {
        self.delta_engine = Some(engine);
        self
    }

    /// 设置 Attention Controller（规则过滤 + 轻量 LLM 排序）
    pub fn with_attention_controller(
        mut self,
        ctrl: crate::component::attention::AttentionController,
    ) -> Self {
        self.attention_controller = Some(ctrl);
        self
    }

    /// 注入人设（CU-5：覆盖默认初始人设）
    pub fn with_persona(mut self, persona: ThreadSafePersona) -> Self {
        self.persona = Some(persona);
        self
    }

    /// 注入事件→特质映射器
    pub fn with_event_trait_mapper(mut self, mapper: std::sync::Arc<std::sync::RwLock<EventTraitMapper>>) -> Self {
        self.event_trait_mapper = Some(mapper);
        self
    }

    pub fn with_persona_store(mut self, store: std::sync::Arc<PersonaStore>) -> Self {
        self.persona_store = Some(store);
        self
    }

    /// 设置情绪配置
    pub fn with_emotion_config(
        mut self,
        config: crate::component::emotion::config::EmotionConfig,
    ) -> Self {
        self.emotion_config = Some(config);
        self
    }

    /// 启用即时事件处理（DB 持久化 + Session Triage LLM 架构）
    ///
    /// 创建 EventStore + ImmediateEventHandler。
    /// SessionTriageEngine 在 lifecycle.rs 的主循环中 spawn。
    pub fn with_immediate_handler(mut self) -> Self {
        use std::sync::Arc;
        use tokio::sync::Notify;

        // 从配置中获取 event_triage 配置
        let triage_config = match self
            .config
            .game_rules
            .as_ref()
            .and_then(|g| g.immediate_events.as_ref())
            .and_then(|e| e.event_triage.clone())
        {
            Some(c) => c,
            None => return self,
        };

        if triage_config.pre_filter.fallback_thresholds().is_err() {
            tracing::error!("event_triage.pre_filter 阈值无效，即时事件处理不可用");
            return self;
        }

        let notify = Arc::new(Notify::new());

        // 创建 EventStore（SQLite）
        let event_store = match EventStore::open(&self.data_dir, &triage_config, notify) {
            Ok(store) => Arc::new(store),
            Err(e) => {
                tracing::error!("创建 EventStore 失败: {}，即时事件处理不可用", e);
                return self;
            }
        };

        // 创建处理器
        let handler = Arc::new(ImmediateEventHandler::new(event_store));

        self.immediate_handler = Some(handler);
        self
    }

    /// 设置数据目录
    pub fn data_dir(mut self, path: PathBuf) -> Self {
        self.data_dir = path;
        self
    }

    /// 构建 Agent
    pub fn build(self) -> Agent {
        let client = AgentClient::new(self.config.server.clone());

        // 设置设备身份
        let device_ref = self
            .device_config
            .as_ref()
            .map(|dc| (dc.device_id, dc.auth_token.clone()));

        if let Some((device_id, auth_token)) = device_ref {
            tokio::task::block_in_place(|| {
                Handle::current().block_on(async {
                    client.set_identity(device_id, auth_token).await;
                });
            });
        }

        // 初始化记忆系统
        let memory_manager = if self.enable_memory {
            let agent_id = self
                .character_config
                .as_ref()
                .and_then(|c| c.agent_id)
                .unwrap_or_else(Uuid::new_v4);
            let config = self.memory_config.unwrap_or_else(|| MemoryManagerConfig {
                agent_id,
                db_dir: self.data_dir.clone(),
                working_memory_size: self.config.memory.working_memory_size,
                episodic_threshold: self.config.memory.episodic_threshold,
                ebbinghaus_config: self.config.memory.ebbinghaus.clone().unwrap_or_default(),
                forgetting_interval_ticks: self.config.memory.forgetting_interval_ticks,
                narrative_min_events: 1,
            });

            // 初始化记忆管理器（使用本地 embedder）
            let result = MemoryManager::new(config);

            match result {
                Ok(manager) => {
                    let agent_name = self
                        .character_config
                        .as_ref()
                        .map(|c| c.name.as_str())
                        .unwrap_or("(未创建)");
                    info!("Memory system initialized for agent '{}'", agent_name);
                    Some(Arc::new(tokio::sync::RwLock::new(manager)))
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

        if let Some(engine) = self.cognitive_engine.as_ref()
            && let Some(ref mm) = memory_manager
        {
            engine.set_memory_manager(mm.clone());
        }

        if let Some(engine) = self.cognitive_engine.as_ref()
            && let Some(ref store) = self.relationship_store
        {
            engine.set_relationship_store(store.clone());
        }

        let persona = self.persona.unwrap_or_else(|| {
            ThreadSafePersona::new(crate::component::persona::DynamicPersona::new(
                Uuid::new_v4(),
                "无名侠客",
                "你是一名行走在江湖中的侠客。",
            ))
        });
        let event_trait_mapper = self.event_trait_mapper.unwrap_or_else(|| {
            let mapper = crate::component::persona::rules_loader::load_event_trait_rules(
                &crate::config::config_dir().join("persona_event_rules.yaml"),
            );
            match mapper {
                Ok(m) => std::sync::Arc::new(std::sync::RwLock::new(m)),
                Err(e) => {
                    warn!("persona_event_rules.yaml 加载失败(等待 Server 推送): {:#}", e);
                    std::sync::Arc::new(std::sync::RwLock::new(EventTraitMapper::new()))
                }
            }
        });

        let agent = Agent {
            config: self.config,
            client,
            decision_callback: self.decision_callback,
            decision_with_memory_callback: self.decision_with_memory_callback,
            decision_with_feedback_callback: self.decision_with_feedback_callback,
            decision_with_chain_callback: self.decision_with_chain_callback,
            memory_manager,
            dialogue_client: self.dialogue_client,
            dialogue_manager: self.dialogue_manager,
            relationship_store: self.relationship_store,
            validator: self.validator,
            last_rejection_reason: None,
            registration_callback: None,
            reconnect_backoff: 0,
            reconnect_rx: self.reconnect_rx,
            death_reported: false,
            rebirth_delay_ticks: 0,
            death_tick_id: None,
            consecutive_llm_failures: 0,
            llm_chaos_active: false,
            actor_llm_container: self.llm_container,
            http_api_state: self.http_api_state,
            device_config: self.device_config,
            character_config: self.character_config,
            data_dir: self.data_dir,
            cognitive_engine: self.cognitive_engine,
            server_assigned_name: None,
            immediate_handler: self.immediate_handler,
            session_triage_handle: None,
            session_triage_game_day: None,
            server_error_feedback: Arc::new(tokio::sync::Mutex::new(None)),
            consecutive_idle_count: 0,
            chaos_generator: self.chaos_generator,
            world_state_store: self.world_state_store,
            delta_engine: self.delta_engine,
            attention_controller: self.attention_controller,
            current_focus_summary: Arc::new(tokio::sync::RwLock::new(None)),
            emotion_config: self.emotion_config,
            core_affect: None,
            current_tick: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
            persona,
            event_trait_mapper,
            persona_store: self.persona_store,
        };

        // CU-5: Engine 需要从 Agent 拿 persona 引用（真相源在 Agent）
        if let Some(ref engine) = agent.cognitive_engine {
            engine.set_persona_ref(std::sync::Arc::new(agent.persona.clone()));
        }

        agent
    }
}
