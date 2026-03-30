// ============================================================================
// AI Module - 智能增强模块
// ============================================================================
//
// 提供各种 AI 相关的功能，包括认知引擎、记忆系统、人设系统等
//
// ## 子模块
// - `cognitive/` - 认知引擎（多阶段决策 + 叙事化）
// - `llm/` - LLM 客户端
// - `memory/` - 记忆系统
// - `persona/` - 人设系统
// - `validator/` - 意图验证
// - `dialogue/` - 对话系统
// - `lifespan/` - 寿命计算
// - `relationship/` - 关系管理
// - `prompts.rs` - Prompt 模板

pub mod dialogue;
pub mod lifespan;
pub mod llm;
pub mod memory;
pub mod persona;
pub mod prompts;
pub mod relationship;
pub mod validator;

// 重导出常用的 LLM 类型
pub use llm::LlmClient;

// 重导出人设系统
pub use persona::{
    dynamic_persona::{DynamicPersona, PersonaState, ThreadSafePersona},
    event_mapper::{EventTraitMapper, TraitMappingRule},
    trait_types::{Trait, TraitChange, TraitType},
};

// 重导出 Prompt 模板
pub use prompts::AgentPrompt;

// 重导出验证器
pub use validator::{
    CognitiveValidator, IntentValidator, PersonaInfo, RejectionType, ValidationRequest,
    ValidationResult, Validator,
};

// 重导出记忆系统
pub use memory::{
    ArchiveMemoryBackend, ClientMemory, EbbinghausConfig, EmbedderService, EpisodicMemoryBackend,
    ForgettingReport, ForgettingScheduler, ImportanceScorer, LocalEmbedder, MemoryEntry,
    MemoryManager, MemoryManagerConfig, MemoryManagerStats, MemoryToolDefinition,
    RecallArchivedParams, SearchMemoryParams, WorkingMemoryBackend,
};

// 重导出关系系统
pub use relationship::{KeyEvent, RelationshipMemory, RelationshipStore};

// 重导出寿命系统
pub use lifespan::{
    AgingEffectValues, AgingEffects, AgingStage, LifespanCalculator, LifespanConfig, LifespanStatus,
};
