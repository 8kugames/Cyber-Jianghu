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
// ├── soul/         # 三魂系统（人魂 + 天魂）
// │   ├── actor/    #   人魂：直连 WorldState，输出结构化 Intent
// │   └── reflector/#   天魂：三层审核（action_type → 规则 → 人设）
// ├── component/    # 共享能力组件
// │   ├── memory/   #   三级记忆系统
// │   ├── persona/  #   身份系统（人设 + 寿命 + 事件演化 + 预设）
// │   ├── social/   #   社交系统（关系 + 对话）
// │   └── llm/      #   LLM 客户端抽象层
// ├── infra/        # 基础设施

#![allow(deprecated)]
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
// ## 三魂架构（人魂→天魂）
// ```
// 人魂 (ActorSoul)           天魂 (ReflectorSoul)
//   直连 WorldState            三层审核
//   action_type=eat         ──→  action_type 合法性
//   action_data={item_id:       RuleEngine 规则校验
//     "mantou"}                  LLM 人设/世界观审查
// ```
// 人魂直连 WorldState，直接输出含精确 ID 的结构化 Intent，
// 天魂对格式化 Intent 进行三层审查。驳回原因通过 last_rejection_reason
// 跨 tick 反馈给人魂，使下一 tick 的决策能参考上一次的驳回理由。

// ============================================================================
// 模块声明
// ============================================================================

// 本地核心逻辑（Agent 组装、生命周期）
pub mod core;

// 三魂系统
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
pub use infra::transport::{
    AgentClient, ConnectError, ServerConfig as TransportServerConfig, WebSocketClient,
};

// 核心
pub use core::{Agent, AgentBuilder};

// 三魂系统
pub use soul::actor::{
    CognitiveChain, CognitiveEngine, CognitiveEngineConfig, CognitiveStage, StageOutput,
};
pub use soul::reflector::rule_engine::{
    Rule, RuleCondition, RuleEngine, RuleEngineConfig, RuleType, RuleValidationContext,
    RuleValidationResult,
};
pub use soul::reflector::{
    ObserverPrompt, PendingReview, PendingReviewEntry, PersonaInfo, ReflectorSoul, RejectionType,
    ReviewDecision, ReviewStatus, ReviewStore, ValidationRequest, ValidationResult, Validator,
    sanitize_for_prompt,
};
// 运行模式
pub use runtime::{
    CognitiveDecisionConfig, DecisionCallback, DecisionWithFeedbackCallback,
    DecisionWithMemoryCallback, HttpApiState, HttpDecisionConfig, HttpDecisionState, IntentRequest,
    claw, cognitive_decision, cognitive_decision_with_chain, create_http_state, http_decision,
    run_http_server,
};

// 组件
pub use component::llm::LlmClient;
pub use component::memory::{
    ClientMemory, EbbinghausConfig, EmbedderService, EpisodicMemoryBackend, ForgettingReport,
    ForgettingScheduler, ImportanceScorer, LocalEmbedder, MemoryEntry, MemoryManager,
    MemoryManagerConfig, MemoryManagerStats, MemoryToolDefinition, RecallArchivedParams,
    SearchMemoryParams, WorkingMemoryBackend,
};
pub use component::persona::{
    AgentPrompt, DynamicPersona, EventTraitMapper, PersonaState, ThreadSafePersona, Trait,
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
