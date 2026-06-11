// ============================================================================
// Cyber-Jianghu Embedding Service
// ============================================================================
// 本地 BERT 向量嵌入（bge-small-zh-v1.5）
// - lib: LocalEmbedder（可被 agent crate 复用）
// - bin: 独立 HTTP 服务（Docker 部署）
// ============================================================================

pub mod download;
pub mod local_embedder;

pub use download::download_model;
pub use local_embedder::{LocalEmbedder, LocalEmbedderConfig};
