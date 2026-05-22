use crate::component::memory::backend::{MemoryBackend, SearchableBackend, SemanticSearchable};
use crate::component::memory::backends::semantic::fts_fallback::FtsFallback;
use crate::component::memory::backends::semantic::vector_store::HnswVectorStore;
use crate::component::memory::embedder::EmbedderService;
use crate::component::memory::store::ClientMemory;
use crate::component::memory::tools::SearchMemoryParams;
use crate::component::memory::types::MemoryEntry;
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

pub struct SemanticMemoryBackend {
    #[allow(dead_code)]
    agent_id: Uuid,
    #[allow(dead_code)]
    config: SemanticMemoryConfig,
    vector_store: Mutex<HnswVectorStore>,
    fts_fallback: Mutex<FtsFallback>,
    /// 可写的 episodic 数据库连接（用于更新 embedding blob）
    episodic_conn: Mutex<rusqlite::Connection>,
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

        // 打开可写的 episodic 数据库连接（用于 embedding 写入）
        let episodic_conn = rusqlite::Connection::open(&config.episodic_db_path)
            .context("Failed to open episodic database for writing")?;

        let use_vector = embedder.is_available() && !vector_store.is_empty();

        Ok(Self {
            agent_id,
            config,
            vector_store: Mutex::new(vector_store),
            fts_fallback: Mutex::new(fts_fallback),
            episodic_conn: Mutex::new(episodic_conn),
            embedder,
            use_vector: Mutex::new(use_vector),
        })
    }

    pub fn is_vector_mode(&self) -> bool {
        *self.use_vector.lock().expect("lock poisoned")
    }

    pub fn fallback_to_fts(&self) {
        *self.use_vector.lock().expect("lock poisoned") = false;
        tracing::info!("Semantic memory fallback to FTS mode");
    }

    pub fn try_upgrade_to_vector(&self) {
        if self.embedder.is_available() {
            *self.use_vector.lock().expect("lock poisoned") = true;
            tracing::info!("Semantic memory upgraded to vector mode");
        }
    }

    pub fn search_fts_only(&self, query: &str, limit: usize) -> Result<Vec<(i64, f32)>> {
        let fts = self.fts_fallback.lock().expect("lock poisoned");
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

        // 尝试 embed：懒加载 embedder，成功则自动升级到 vector 模式
        match self.embedder.embed(&params.query).await {
            Ok(vector) => {
                if !self.is_vector_mode() {
                    self.try_upgrade_to_vector();
                }

                // HnswVectorStore::search() 内部处理 rebuild，无需重复
                let mut vector_store = self.vector_store.lock().expect("lock poisoned");
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
                tracing::debug!("Embedding unavailable: {}, using FTS fallback", e);
            }
        }

        tracing::debug!("Using FTS fallback for query: {}", params.query);
        let fts = self.fts_fallback.lock().expect("lock poisoned");
        let results = fts.search(&params.query, params.limit)?;
        Ok(results
            .into_iter()
            .map(|m| (m.id.unwrap_or(0), 0.0))
            .collect())
    }

    fn client_memory_to_entry(memory: ClientMemory) -> MemoryEntry {
        let mut entry = MemoryEntry::new(memory.agent_id, memory.tick_id, memory.content)
            .with_event_type(memory.event_type)
            .with_importance(memory.importance_score)
            .with_metadata(memory.metadata);

        entry.id = memory.id;
        entry.strength = memory.strength;
        entry.is_archived = memory.is_archived;
        entry.access_count = memory.access_count as u32;
        entry.last_accessed_at = memory
            .last_accessed_at
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        entry
    }
}

#[async_trait]
impl MemoryBackend for SemanticMemoryBackend {
    fn name(&self) -> &'static str {
        "SemanticMemory"
    }

    async fn add(&mut self, memory: &mut MemoryEntry) -> Result<i64> {
        // 获取 memory ID（episodic.add() 已设置）
        let Some(mem_id) = memory.id else {
            // ID 未设置，跳过（semantic 层无法独立关联 embedding）
            tracing::debug!("SemanticMemory::add() skipped: memory.id not set");
            return Ok(-1);
        };

        // 生成 embedding（embedder 不可用时传播错误，不静默跳过）
        let embedding = match self.embedder.embed(&memory.content).await {
            Ok(e) => e,
            Err(e) => {
                anyhow::bail!("Failed to generate embedding for memory {}: {}", mem_id, e);
            }
        };

        // 写入 embedding blob 到 episodic DB
        let encoded = HnswVectorStore::encode_vector(&embedding);
        {
            let conn = self.episodic_conn.lock().expect("lock poisoned");
            conn.execute(
                "UPDATE client_memories SET embedding = ?1 WHERE id = ?2",
                rusqlite::params![encoded, mem_id],
            )
            .context("Failed to write embedding to episodic DB")?;
        }

        // 添加到 HNSW 内存索引
        {
            let mut vs = self.vector_store.lock().expect("lock poisoned");
            vs.add(mem_id, embedding)?;
        }

        // 标记使用向量模式
        if !self.is_vector_mode() {
            self.try_upgrade_to_vector();
        }

        Ok(mem_id)
    }

    async fn count(&self) -> Result<usize> {
        Ok(self.vector_store.lock().expect("lock poisoned").len())
    }

    async fn clear(&mut self) -> Result<()> {
        self.vector_store.lock().expect("lock poisoned").clear();
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
        // 尝试 embed：懒加载 embedder，成功则自动升级到 vector 模式
        if let Ok(query_vector) = self.embedder.embed(query).await {
            if !self.is_vector_mode() {
                self.try_upgrade_to_vector();
            }

            let results = self
                .vector_store
                .lock()
                    .expect("lock poisoned")
                .search(&query_vector, limit);

            match results {
                Ok(results) if !results.is_empty() => {
                    tracing::info!("Vector search returned {} results", results.len());
                    let fts = self.fts_fallback.lock().expect("lock poisoned");
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
            tracing::debug!("Embedding unavailable, using FTS fallback");
        }

        let fts = self.fts_fallback.lock().expect("lock poisoned");
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
