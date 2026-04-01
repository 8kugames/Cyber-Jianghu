// ============================================================================
// Cyber-Jianghu Agent SDK
// ============================================================================
//
// 用于连接 Cyber-Jianghu MMO-MAS 服务端的 Agent SDK
//
// ## 目录结构
// ```
// crates/agent/src/
// ├── core/         # Agent 结构 + 生命周期（编排者）
// ├── soul/         # 双魂系统（ActorSoul + ReflectorSoul）
// │   ├── actor/    #   认知引擎 + 叙事化
// │   └── reflector/#   意图验证 + 审查存储
// ├── component/    # 共享能力组件
// │   ├── memory/   #   三级记忆系统
// │   ├── persona/  #   身份系统（人设 + 寿命 + 事件演化 + 预设）
// │   ├── social/   #   社交系统（关系 + 对话）
// │   └── llm/      #   LLM 客户端抽象层
// ├── infra/        # 基础设施
// │   ├── api/      #   HTTP API 服务器
// │   └── transport/#   游戏服务器 WebSocket 客户端
// ├── runtime/      # 模式入口（cognitive / claw）
// ├── config.rs     # 配置（数据）
// ├── models.rs     # 数据模型
// └── bin/          # CLI 入口（组装）
// ```
//
// ## 数据流
// ```
// Server ─[WebSocket]→ Transport ─[WorldState]→ Runtime ─[Intent]→ Transport ─[WebSocket]→ Server
// ```
//
// ## 双魂架构（同步审查）
// ```
// ActorSoul（行动之魂/本我）     ReflectorSoul（反思之魂/超我）
//        │                              │
//        │  generate_intent()           │
//        │  ─────────────────────────> │  validate_with_reflector()
//        │                              │  LLM 同步审查（单次调用）
//        │  approved → send_intent()    │
//        │  rejected → idle + 反馈      │
//        ▼                              │
//    send_intent()                     │
// ```
// 驳回原因通过 last_rejection_reason 跨 tick 反馈给 ActorSoul，
// 使下一 tick 的决策能参考上一次的驳回理由。

// ============================================================================
// 模块声明
// ============================================================================

// 本地核心逻辑（Agent 组装、生命周期）
pub mod core;

// 双魂系统
pub mod soul;

// 共享能力组件
pub mod component;

// 基础设施
pub mod infra;

// 运行模式（决策函数）
pub mod runtime;

// 配置和数据模型
pub mod config;
pub mod models;

// ============================================================================
// 重导出常用类型
// ============================================================================

// 通信层
pub use infra::transport::{AgentClient, ConnectError, ServerConfig as TransportServerConfig, WebSocketClient};

// 核心
pub use core::{Agent, AgentBuilder};

// 双魂系统
pub use soul::actor::{
    CognitiveChain, CognitiveEngineConfig, CognitiveStage, MultiStageCognitiveEngine, StageOutput,
};
pub use soul::reflector::rule_engine::RuleEngine as RuleEngineValidator;
pub use soul::reflector::rule_engine::{
    Rule, RuleCondition, RuleEngine, RuleEngineConfig, RuleType, RuleValidationContext,
    RuleValidationResult,
};
pub use soul::reflector::{
    ObserverPrompt, PendingReview, PendingReviewEntry, PersonaInfo,
    ReflectorSoul, RejectionType, ReviewDecision, ReviewStatus, ReviewStore, ValidationRequest,
    ValidationResult, Validator, sanitize_for_prompt,
};

// 运行模式
pub use runtime::{
    CognitiveDecisionConfig, DecisionCallback, DecisionWithFeedbackCallback,
    DecisionWithMemoryCallback, HttpApiState, HttpDecisionConfig, HttpDecisionState, IntentRequest,
    claw, cognitive_decision, cognitive_decision_with_retry, create_http_state, http_decision,
    run_http_server,
};

// 组件
pub use component::llm::LlmClient;
pub use component::memory::{
    ArchiveMemoryBackend, ClientMemory, EbbinghausConfig, EmbedderService, EpisodicMemoryBackend,
    ForgettingReport, ForgettingScheduler, ImportanceScorer, LocalEmbedder, MemoryEntry,
    MemoryManager, MemoryManagerConfig, MemoryManagerStats, MemoryToolDefinition,
    RecallArchivedParams, SearchMemoryParams, WorkingMemoryBackend,
};
pub use component::persona::{
    AgentPrompt, AgingEffectValues, AgingEffects, AgingStage, DynamicPersona, EventTraitMapper,
    LifespanCalculator, LifespanConfig, LifespanStatus, PersonaState, ThreadSafePersona, Trait,
    TraitChange, TraitMappingRule, TraitType, get_agent_prompt, get_all_agent_prompts,
};
pub use component::social::{KeyEvent, RelationshipMemory, RelationshipStore};

// 配置
pub use config::{
    AgentRole, CharacterConfig, Config, DeviceConfig, GoalsConfig, LanguageStyleConfig,
    MemoryConfig, ReviewConfig, RuntimeConfig, RuntimeMode, ServerConfig,
};
pub use models::{ActionType, Intent, WorldEvent, WorldState};

// 错误类型（从 common 合并)
pub use cyber_jianghu_protocol::GameError;

/// SDK 版本
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// SDK 名称
pub const NAME: &str = "cyber-jianghu-agent";
