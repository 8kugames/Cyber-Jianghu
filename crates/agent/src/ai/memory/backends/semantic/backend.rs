use crate::ai::memory::backend::{MemoryBackend, SearchableBackend, SemanticSearchable};
use crate::ai::memory::backends::semantic::fts_fallback::FtsFallback;
use crate::ai::memory::backends::semantic::vector_store::HnswVectorStore;
use crate::ai::memory::embedder::EmbedderService;
use crate::ai::memory::store::ClientMemory;
use crate::ai::memory::tools::SearchMemoryParams;
use crate::ai::memory::types::MemoryEntry;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub struct SemanticMemoryConfig {
    pub dimension: usize,
    pub db_path: PathBuf,
    pub episodic_db_path: PathBuf,
    pub embedding_threshold: f32,
}

impl Default for SemanticMemoryConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let data_dir = home.join(".cyber-jianghu").join("data");
        Self {
            dimension: 512,
            db_path: data_dir.join("semantic.db"),
            episodic_db_path: data_dir.join("episodic.db"),
            embedding_threshold: 0.7,
        }
    }
}

pub struct SemanticMemoryBackend {
    #[allow(dead_code)]
    agent_id: Uuid,
    #[allow(dead_code)]
    config: SemanticMemoryConfig,
    vector_store: Mutex<HnswVectorStore>,
    fts_fallback: Mutex<FtsFallback>,
    embedder: Arc<EmbedderService>,
    use_vector: Mutex<bool>,
}

impl SemanticMemoryBackend {
    pub fn new(
        agent_id: Uuid,
        config: SemanticMemoryConfig,
        embedder: Arc<EmbedderService>,
    ) -> Result<Self> {
        let mut vector_store = HnswVectorStore::new(config.dimension);

        if config.db_path.exists() {
            let conn = rusqlite::Connection::open(&config.db_path)?;
            if let Err(e) = vector_store.load_from_db(&conn) {
                tracing::warn!("Failed to load vectors from database: {}", e);
            }
        }

        let fts_db_path = config
            .db_path
            .parent()
            .map(|p| p.join(format!("{}_fts.db", agent_id)))
            .unwrap_or_else(|| config.db_path.with_extension("fts.db"));

        let fts_fallback = FtsFallback::new(agent_id, &fts_db_path, &config.episodic_db_path)
            .context("Failed to initialize FTS fallback")?;

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

    pub fn is_vector_mode(&self) -> bool {
        *self.use_vector.lock().unwrap()
    }

    pub fn fallback_to_fts(&self) {
        *self.use_vector.lock().unwrap() = false;
        tracing::info!("Semantic memory fallback to FTS mode");
    }

    pub fn try_upgrade_to_vector(&self) {
        if self.embedder.is_available() {
            *self.use_vector.lock().unwrap() = true;
            tracing::info!("Semantic memory upgraded to vector mode");
        }
    }

    pub fn search_fts_only(&self, query: &str, limit: usize) -> Result<Vec<(i64, f32)>> {
        let fts = self.fts_fallback.lock().unwrap();
        let results = fts.search(query, limit)?;
        Ok(results
            .into_iter()
            .map(|m| (m.id.unwrap_or(0), 0.0))
            .collect())
    }

    #[allow(dead_code)]
    async fn generate_embedding(&self, memory: &MemoryEntry) -> Result<Vec<f32>> {
        self.embedder.embed(&memory.content).await
    }

    pub async fn ensure_embeddings_for_priority(&self) -> Result<usize> {
        Ok(0)
    }

    pub async fn search(&self, params: &SearchMemoryParams) -> Result<Vec<(i64, f32)>> {
        if params.query.is_empty() {
            return Ok(Vec::new());
        }

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

        tracing::debug!("Using FTS fallback for query: {}", params.query);
        let fts = self.fts_fallback.lock().unwrap();
        let results = fts.search(&params.query, params.limit)?;
        Ok(results
            .into_iter()
            .map(|m| (m.id.unwrap_or(0), 0.0))
            .collect())
    }

    fn client_memory_to_entry(memory: ClientMemory) -> MemoryEntry {
        MemoryEntry::new(memory.agent_id, memory.tick_id, memory.content)
            .with_event_type(memory.event_type)
            .with_importance(memory.importance_score)
            .with_metadata(memory.metadata)
    }
}

#[async_trait]
impl MemoryBackend for SemanticMemoryBackend {
    fn name(&self) -> &'static str {
        "SemanticMemory"
    }

    async fn add(&mut self, _memory: MemoryEntry) -> Result<()> {
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
        Ok(Vec::new())
    }

    async fn get_recent(&self, _limit: usize) -> Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }

    async fn get_by_tick_range(&self, _start: i64, _end: i64) -> Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl SemanticSearchable for SemanticMemoryBackend {
    async fn search_similar(&mut self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let use_vector = self.is_vector_mode();

        if use_vector {
            if let Ok(query_vector) = self.embedder.embed(query).await {
                let results = self
                    .vector_store
                    .lock()
                    .unwrap()
                    .search(&query_vector, limit);

                match results {
                    Ok(results) if !results.is_empty() => {
                        tracing::debug!("Vector search returned {} results", results.len());
                        let fts = self.fts_fallback.lock().unwrap();
                        let ids: Vec<i64> = results.into_iter().map(|(id, _)| id).collect();
                        let memories = fts.get_memories_by_ids(&ids)?;
                        return Ok(memories
                            .into_iter()
                            .map(Self::client_memory_to_entry)
                            .collect());
                    }
                    Ok(_) => {
                        tracing::debug!("Vector search returned empty, falling back to FTS");
                    }
                    Err(e) => {
                        tracing::warn!("Vector search failed: {}, falling back to FTS", e);
                    }
                }
            } else {
                tracing::warn!("Vector embedding failed, falling back to FTS");
            }
        }

        let fts = self.fts_fallback.lock().unwrap();
        let memories = fts.search(query, limit)?;
        tracing::debug!("FTS search returned {} results", memories.len());

        Ok(memories
            .into_iter()
            .map(Self::client_memory_to_entry)
            .collect())
    }

    async fn ensure_embedding(&mut self, _memory_id: i64) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = SemanticMemoryConfig::default();
        assert_eq!(config.dimension, 512);
        assert_eq!(config.embedding_threshold, 0.7);
    }

    #[test]
    fn test_semantic_memory_backend_creation() {
        let config = SemanticMemoryConfig::default();
        assert!(config.db_path.to_string_lossy().contains("cyber-jianghu"));
    }
}
