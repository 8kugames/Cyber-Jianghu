// ============================================================================
// AI Module - 兼容层
// ============================================================================
//
// 核心子模块已迁移至 component/ 和 soul/ 目录
// 此模块仅保留 prompts 和兼容性重导出
// ============================================================================

pub mod cognitive;
pub mod prompts;

// 向后兼容：从 component/ 重导出
pub use crate::component::llm::LlmClient;
pub use crate::component::memory::{
    ArchiveMemoryBackend, ClientMemory, EbbinghausConfig, EmbedderService, EpisodicMemoryBackend,
    ForgettingReport, ForgettingScheduler, ImportanceScorer, LocalEmbedder, MemoryEntry,
    MemoryManager, MemoryManagerConfig, MemoryManagerStats, MemoryToolDefinition,
    RecallArchivedParams, SearchMemoryParams, WorkingMemoryBackend,
};
pub use crate::component::persona::{
    AgingEffectValues, AgingEffects, AgingStage, DynamicPersona, EventTraitMapper,
    LifespanCalculator, LifespanConfig, LifespanStatus, PersonaState, ThreadSafePersona, Trait,
    TraitChange, TraitMappingRule, TraitType,
};
pub use crate::component::social::{KeyEvent, RelationshipMemory, RelationshipStore};

// 从 soul::reflector 重导出验证器类型
pub use crate::soul::reflector::{
    CognitiveValidator, IntentValidator, PersonaInfo, RejectionType, ValidationRequest,
    ValidationResult, Validator,
};

// 本地模块
pub use prompts::{AgentPrompt, get_agent_prompt, get_all_agent_prompts};
