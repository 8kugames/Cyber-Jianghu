// ============================================================================
// 客户端记忆系统模块
// ============================================================================
//
// 实现三级记忆体系：
// - 工作记忆（WorkingMemoryBackend）：最近 N 条事件，用于 LLM 上下文构建
// - 情景记忆（EpisodicMemoryBackend）：重要事件，长期 SQLite 存储
// - 归档记忆（ArchiveMemoryBackend）：已遗忘的记忆，支持努力回忆
// - 语义记忆（SemanticMemoryBackend）：向量检索（Phase 2）
//
// 设计原则：
// 1. 客户端自主管理记忆，符合"天道无为"原则
// 2. 记忆完全本地化，服务端无法访问
// 3. 支持重要性评分和艾宾浩斯遗忘机制
// ============================================================================

pub mod backend;
pub mod backends;
pub mod embedder;
pub mod forgetting;
pub mod local_embedder;
pub mod manager;
pub mod registry;
pub mod scorer;
pub mod store;
pub mod tools;
pub mod types;

// 重导出常用类型
pub use backend::{ForgettableBackend, MemoryBackend, SearchableBackend, SemanticSearchable};
pub use embedder::EmbedderService;
pub use forgetting::ForgettingScheduler;
pub use local_embedder::LocalEmbedder;
pub use scorer::ImportanceScorer;
pub use registry::{AgentLifetime, GlobalMemoryRegistry, GlobalMemoryReport};
pub use manager::{MemoryManager, MemoryManagerConfig, MemoryManagerStats};
pub use store::{ClientMemory, MemoryStore};
pub use tools::{
    MemoryToolDefinition, MemoryToolResult, MemorySearchResult,
    RecallArchivedParams, SearchMemoryParams,
    RECALL_ARCHIVED_TOOL, SEARCH_MEMORY_TOOL,
};
pub use types::{EbbinghausConfig, EmbedderStatus, ForgettingReport, MemoryEntry};

// 重导出 backends 类型
pub use backends::{
    ArchiveMemoryBackend, EpisodicMemoryBackend, WorkingMemoryBackend,
    // 语义记忆 (Phase 2)
    FtsFallback, HnswVectorStore, SemanticMemoryBackend,
};
