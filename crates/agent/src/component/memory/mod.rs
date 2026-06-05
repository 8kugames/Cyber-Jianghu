// ============================================================================
// 三级记忆系统
// ============================================================================

pub mod backend;
pub mod backends;
pub mod embedder;
pub mod forgetting;
pub mod local_embedder;
pub mod manager;
pub mod outcome;
pub mod registry;
pub mod scorer;
pub mod store;
pub mod tools;
pub mod types;

pub use backend::{ForgettableBackend, MemoryBackend, SearchableBackend, SemanticSearchable};
pub use backends::{
    EpisodicMemoryBackend, FtsFallback, HnswVectorStore, SemanticMemoryBackend,
    WorkingMemoryBackend,
};
pub use embedder::EmbedderService;
pub use forgetting::ForgettingScheduler;
pub use local_embedder::LocalEmbedder;
pub use manager::{MemoryManager, MemoryManagerConfig, MemoryManagerStats};
pub use outcome::{OutcomeMemory, OutcomeRecord, OutcomeResult, compute_context_hash, extract_target_agent_id};
pub use registry::{AgentLifetime, GlobalMemoryRegistry, GlobalMemoryReport};
pub use scorer::ImportanceScorer;
pub use store::{ClientMemory, MemoryStore};
pub use tools::{
    MemorySearchResult, MemoryToolDefinition, MemoryToolResult, RECALL_ARCHIVED_TOOL,
    RecallArchivedParams, SEARCH_MEMORY_TOOL, SearchMemoryParams,
};
pub use types::{EbbinghausConfig, EmbedderStatus, ForgettingReport, MemoryEntry};
