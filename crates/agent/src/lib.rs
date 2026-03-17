// ============================================================================
// Cyber-Jianghu Agent SDK
// ============================================================================
//
// 用于连接 Cyber-Jianghu MMO-MAS 服务端的 Agent SDK
//
// ## COI 架构（组合优于继承）
// ```
// crates/agent/src/
// ├── transport/    # 与 server 通信的 SDK（纯 I/O）
// ├── core/         # 本地核心逻辑（Agent 组装、生命周期）
// ├── runtime/      # 运行模式（各种决策函数）
// ├── ai/           # 智能增强模块
// │   ├── llm/       # LLM 客户端
// │   ├── cognitive/ # 认知引擎 + 叙事化
// │   ├── memory/    # 记忆系统
// │   ├── persona/   # 人设系统
// │   ├── skill/     # 技能系统
//   ├── validator/ # 意图验证
//   ├── dialogue/   # 对话系统
//   ├── lifespan/   # 寿命计算
//   ├── relationship/ # 关系管理
//   └── prompts.rs   # Prompt 模板
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
// ## 运行模式（runtime 模块提供）
// - `simple`: 简单规则决策（基于生理需求）
// - `idle`: 只空闲
// - `stdio`: 标准输入输出（外部程序决策）
// - `tcp`: TCP 服务器（外部程序决策）
// - `http`: HTTP API 服务器（外部程序决策）
// - `cognitive`: 多阶段认知引擎（内置 LLM 决策）

// ============================================================================
// 模块声明
// ============================================================================

// 与 server 通信的 SDK（纯 I/O）
pub mod transport;

// 本地核心逻辑（Agent 组装、生命周期）
pub mod core;

// 运行模式（决策函数）
pub mod runtime;

// 智能增强模块
pub mod ai;

// 配置和数据模型
pub mod config;
pub mod models;

// ============================================================================
// 重导出常用类型
// ============================================================================

// 通信层
pub use transport::{WebSocketClient, AgentClient, ServerConfig as TransportServerConfig};

// 核心
pub use core::{Agent, AgentBuilder};

// 运行模式
pub use runtime::{
    DecisionCallback,
    http_decision, HttpDecisionConfig, HttpDecisionState, HttpApiState, create_http_state, run_http_server, IntentRequest,
    cognitive_decision, cognitive_decision_with_retry, CognitiveDecisionConfig,
    DecisionWithFeedbackCallback, DecisionWithMemoryCallback,
};

// AI 模块
pub use ai::llm::LlmClient;
pub use ai::persona::{
    dynamic_persona::{DynamicPersona, PersonaState, ThreadSafePersona},
    event_mapper::{EventTraitMapper, TraitMappingRule},
    trait_types::{Trait, TraitChange, TraitType},
};
pub use ai::prompts::{get_agent_prompt, get_all_agent_prompts, AgentPrompt};
pub use ai::validator::{
    IntentValidator, PersonaInfo, RejectionType, ValidationRequest, ValidationResult, Validator,
};
pub use ai::memory::{
    ArchiveMemoryBackend, ClientMemory, EmbedderService,
    EpisodicMemoryBackend, ForgettingScheduler, ImportanceScorer,
    LocalEmbedder, MemoryEntry, MemoryManager, MemoryManagerConfig, MemoryManagerStats,
    MemoryToolDefinition, SearchMemoryParams, RecallArchivedParams,
    WorkingMemoryBackend,
    EbbinghausConfig, ForgettingReport,
};
pub use ai::relationship::{KeyEvent, RelationshipMemory, RelationshipStore};
pub use ai::lifespan::{
    AgingEffectValues, AgingEffects, AgingStage, LifespanCalculator, LifespanConfig, LifespanStatus,
};

// 配置
pub use config::{AgentConfig, Config, ServerConfig};
pub use models::{ActionType, Intent, WorldEvent, WorldState};

// 错误类型（从 common 合并)
pub use cyber_jianghu_protocol::GameError;

/// SDK 版本
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// SDK 名称
pub const NAME: &str = "cyber-jianghu-agent";
