// ============================================================================
// 语义记忆后端实现
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 提供向量语义检索功能，支持 FTS 降级
// ============================================================================

use crate::ai::memory::backend::{MemoryBackend, SearchableBackend, SemanticSearchable};
use crate::ai::memory::backends::semantic::fts_fallback::FtsFallback;
use crate::ai::memory::backends::semantic::vector_store::HnswVectorStore;
use crate::ai::memory::embedder::EmbedderService;
use crate::ai::memory::tools::SearchMemoryParams;
use crate::ai::memory::types::MemoryEntry;
use anyhow::{Context, Result};
use async_trait::async_trait;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// 语义记忆后端配置
pub struct SemanticMemoryConfig {
    /// 向量维度
    pub dimension: usize,
    /// 数据库路径
    pub db_path: PathBuf,
    /// 重要性阈值（只为此阈值以上的记忆生成向量）
    pub embedding_threshold: f32,
}

impl Default for SemanticMemoryConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            dimension: 512,
            db_path: home.join(".cyber-jianghu").join("data").join("semantic.db"),
            embedding_threshold: 0.7,
        }
    }
}

/// 语义记忆后端 - 向量语义检索
///
/// 支持向量检索和 FTS 降级
pub struct SemanticMemoryBackend {
    /// Agent ID
    #[allow(dead_code)]
    agent_id: Uuid,
    /// 配置
    #[allow(dead_code)]
    config: SemanticMemoryConfig,
    /// 向量存储
    vector_store: Mutex<HnswVectorStore>,
    /// FTS 降级
    fts_fallback: Mutex<FtsFallback>,
    /// 嵌入服务
    embedder: Arc<EmbedderService>,
    /// 是否使用向量模式
    use_vector: Mutex<bool>,
}

impl SemanticMemoryBackend {
    /// 创建新的语义记忆后端
    pub fn new(
        agent_id: Uuid,
        config: SemanticMemoryConfig,
        embedder: Arc<EmbedderService>,
    ) -> Result<Self> {
        // 初始化向量存储
        let mut vector_store = HnswVectorStore::new(config.dimension);

        // 尝试从数据库加载现有向量
        if config.db_path.exists() {
            let conn = Connection::open(&config.db_path)?;
            if let Err(e) = vector_store.load_from_db(&conn) {
                tracing::warn!("Failed to load vectors from database: {}", e);
            }
        }

        // 初始化 FTS 降级
        let fts_db_path = config
            .db_path
            .parent()
            .map(|p| p.join(format!("{}_fts.db", agent_id)))
            .unwrap_or_else(|| config.db_path.with_extension("fts.db"));

        let fts_fallback = FtsFallback::new(agent_id, &fts_db_path)
            .context("Failed to initialize FTS fallback")?;

        // 检查是否可用向量模式
        let use_vector = embedder.is_available() && !vector_store.is_empty();

        Ok(Self {
            agent_id,
            config,
            vector_store: Mutex::new(vector_store),
            fts_fallback: Mutex::new(fts_fallback),
            embedder,
            use_vector: Mutex::new(use_vector),
        })
    }

    /// 检查是否使用向量模式
    pub fn is_vector_mode(&self) -> bool {
        *self.use_vector.lock().unwrap()
    }

    /// 降级到 FTS 模式
    pub fn fallback_to_fts(&self) {
        *self.use_vector.lock().unwrap() = false;
        tracing::info!("Semantic memory fallback to FTS mode");
    }

    /// 尝试升级到向量模式
    pub fn try_upgrade_to_vector(&self) {
        if self.embedder.is_available() {
            *self.use_vector.lock().unwrap() = true;
            tracing::info!("Semantic memory upgraded to vector mode");
        }
    }

    /// 搜索记忆（FTS 降级测试）
    pub fn search_fts_only(&self, query: &str, limit: usize) -> Result<Vec<(i64, f32)>> {
        let fts = self.fts_fallback.lock().unwrap();
        let results = fts.search(query, limit)?;

        // FTS 结果没有相似度分数，默认为 0.0
        Ok(results.into_iter().map(|id| (id, 0.0)).collect())
    }

    /// 为记忆生成嵌入向量
    #[allow(dead_code)]
    async fn generate_embedding(&self, memory: &MemoryEntry) -> Result<Vec<f32>> {
        self.embedder.embed(&memory.content).await
    }

    /// 为高重要性记忆生成向量（按需策略）
    pub async fn ensure_embeddings_for_priority(&self) -> Result<usize> {
        // 获取没有向量的高重要性记忆
        // 这是简化实现，实际需要从 EpisodicMemoryBackend 获取
        Ok(0)
    }

    /// 搜索记忆
    pub async fn search(&self, params: &SearchMemoryParams) -> Result<Vec<(i64, f32)>> {
        // 如果查询文本为空，返回空结果
        if params.query.is_empty() {
            return Ok(Vec::new());
        }

        // 尝试使用向量检索
        if self.is_vector_mode() {
            match self.embedder.embed(&params.query).await {
                Ok(vector) => {
                    let vector_store = self.vector_store.lock().unwrap();
                    match vector_store.search(&vector, params.limit) {
                        Ok(results) => {
                            tracing::debug!("Vector search returned {} results", results.len());
                            return Ok(results);
                        }
                        Err(e) => {
                            tracing::warn!("Vector search failed: {}, falling back to FTS", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to embed query: {}, falling back to FTS", e);
                }
            }
        }

        // 降级到 FTS
        tracing::debug!("Using FTS fallback for query: {}", params.query);
        let fts = self.fts_fallback.lock().unwrap();
        let results = fts.search(&params.query, params.limit)?;

        // FTS 结果没有相似度分数，默认为 0.0
        Ok(results.into_iter().map(|id| (id, 0.0)).collect())
    }
}

#[async_trait]
impl MemoryBackend for SemanticMemoryBackend {
    fn name(&self) -> &'static str {
        "SemanticMemory"
    }

    async fn add(&mut self, _memory: MemoryEntry) -> Result<()> {
        // 语义后端不直接添加记忆
        // 记忆由 EpisodicMemoryBackend 添加，然后按需生成向量
        Ok(())
    }

    async fn count(&self) -> Result<usize> {
        Ok(self.vector_store.lock().unwrap().len())
    }

    async fn clear(&mut self) -> Result<()> {
        self.vector_store.lock().unwrap().clear();
        Ok(())
    }
}

#[async_trait]
impl SearchableBackend for SemanticMemoryBackend {
    async fn get_top_by_importance(&self, _limit: usize) -> Result<Vec<MemoryEntry>> {
        // 语义后端不按重要性排序
        // 由 EpisodicMemoryBackend 提供
        Ok(Vec::new())
    }

    async fn get_recent(&self, _limit: usize) -> Result<Vec<MemoryEntry>> {
        // 语义后端不按时间排序
        Ok(Vec::new())
    }

    async fn get_by_tick_range(&self, _start: i64, _end: i64) -> Result<Vec<MemoryEntry>> {
        // 语义后端不支持 tick 范围查询
        Ok(Vec::new())
    }
}

#[async_trait]
impl SemanticSearchable for SemanticMemoryBackend {
    async fn search_similar(&mut self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let use_vector = self.is_vector_mode();

        if use_vector {
            // 使用向量搜索
            if let Ok(query_vector) = self.embedder.embed(query).await {
                let results = self
                    .vector_store
                    .lock()
                    .unwrap()
                    .search(&query_vector, limit);

                match results {
                    Ok(results) => {
                        // TODO: 从数据库加载完整的 MemoryEntry
                        tracing::debug!("Vector search returned {} results", results.len());
                        return Ok(Vec::new()); // 简化实现
                    }
                    Err(e) => {
                        tracing::warn!("Vector search failed: {}, falling back to FTS", e);
                    }
                }
            } else {
                tracing::warn!("Vector embedding failed, falling back to FTS");
            }
        }

        // 使用 FTS 降级
        let fts = self.fts_fallback.lock().unwrap();
        let ids = fts.search(query, limit)?;
        tracing::debug!("FTS search returned {} results", ids.len());

        // TODO: 从数据库加载完整的 MemoryEntry
        Ok(Vec::new())
    }

    async fn ensure_embedding(&mut self, _memory_id: i64) -> Result<()> {
        // TODO: 为指定记忆生成嵌入向量
        Ok(())
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::ai::memory::local_embedder::LocalEmbedder;

    // 注意：完整测试需要 mock LlmClient
    // 这里只测试基本结构

    #[test]
    fn test_config_default() {
        let config = SemanticMemoryConfig::default();
        assert_eq!(config.dimension, 512);
        assert_eq!(config.embedding_threshold, 0.7);
    }

    #[test]
    fn test_semantic_memory_backend_creation() {
        // 测试配置创建
        let config = SemanticMemoryConfig::default();
        assert!(config.db_path.to_string_lossy().contains("cyber-jianghu"));
    }
}
