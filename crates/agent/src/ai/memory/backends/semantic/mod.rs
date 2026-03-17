// ============================================================================
// 语义记忆后端
// ============================================================================
//
// 提供向量语义检索功能，支持 FTS 降级
// ============================================================================

pub mod backend;
pub mod vector_store;
pub mod fts_fallback;

pub use backend::SemanticMemoryBackend;
pub use vector_store::HnswVectorStore;
pub use fts_fallback::FtsFallback;
