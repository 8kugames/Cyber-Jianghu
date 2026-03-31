// ============================================================================
// 记忆后端实现
// ============================================================================
//
// 提供三种记忆后端：
// - WorkingMemoryBackend: RAM FIFO 队列，存储最近 N 条事件
// - EpisodicMemoryBackend: SQLite 持久化，支持遗忘机制
// - ArchiveMemoryBackend: 遗忘归档存储
//
// 架构原则：COI（组合优于继承），插件式后端
// ============================================================================

pub mod archive;
pub mod episodic;
pub mod working;
// 语义记忆 (Phase 2)
pub mod semantic;

// 重导出
pub use archive::ArchiveMemoryBackend;
pub use episodic::EpisodicMemoryBackend;
pub use semantic::{FtsFallback, HnswVectorStore, SemanticMemoryBackend};
pub use working::WorkingMemoryBackend;
