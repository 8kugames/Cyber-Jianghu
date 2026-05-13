// ============================================================================
// 情景记忆后端（SQLite + 遗忘）
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 重要事件的长期存储，支持艾宾浩斯遗忘机制
// ============================================================================

use crate::component::memory::backend::{ForgettableBackend, MemoryBackend, SearchableBackend};
use crate::component::memory::store::{ClientMemory, MemoryStore};
use crate::component::memory::types::MemoryEntry;
use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

/// 情景记忆后端
///
/// SQLite 持久化存储，支持遗忘机制
pub struct EpisodicMemoryBackend {
    /// 底层存储（使用 Mutex 包装以支持 Sync）
    store: Mutex<MemoryStore>,
    /// 记忆保存阈值（高于此值的记忆才会被保存）
    save_threshold: f32,
}

impl EpisodicMemoryBackend {
    /// 创建新的情景记忆后端（使用默认阈值 0.5）
    pub fn new(agent_id: Uuid, db_dir: &Path) -> Result<Self> {
        Self::with_threshold(agent_id, db_dir, 0.5)
    }

    /// 创建新的情景记忆后端（使用自定义阈值）
    pub fn with_threshold(agent_id: Uuid, db_dir: &Path, threshold: f32) -> Result<Self> {
        let store = MemoryStore::new(agent_id, db_dir)?;
        Ok(Self {
            store: Mutex::new(store),
            save_threshold: threshold,
        })
    }

    /// 获取记忆保存阈值
    pub fn threshold(&self) -> f32 {
        self.save_threshold
    }

    /// 设置记忆保存阈值
    pub fn set_threshold(&mut self, threshold: f32) {
        self.save_threshold = threshold;
    }

    /// 将 MemoryEntry 转换为 ClientMemory（引用，避免 clone）
    fn entry_to_memory(entry: &MemoryEntry) -> ClientMemory {
        let mut memory = ClientMemory::new(entry.agent_id, entry.tick_id, entry.content.clone())
            .with_type(entry.event_type.clone())
            .with_importance(entry.importance_score)
            .with_metadata(entry.metadata.clone());

        memory.strength = entry.strength;
        memory.is_archived = entry.is_archived;
        memory.access_count = entry.access_count as i32;
        memory.last_accessed_at = entry.last_accessed_at.map(|dt| dt.to_rfc3339());
        memory
    }

    /// 将 ClientMemory 转换为 MemoryEntry
    fn memory_to_entry(memory: &ClientMemory) -> MemoryEntry {
        let mut entry = MemoryEntry::new(memory.agent_id, memory.tick_id, memory.content.clone())
            .with_event_type(memory.event_type.clone())
            .with_importance(memory.importance_score)
            .with_metadata(memory.metadata.clone());

        // 搬运遗忘机制关键字段
        entry.strength = memory.strength;
        entry.is_archived = memory.is_archived;
        entry.access_count = memory.access_count as u32;
        entry.last_accessed_at = memory
            .last_accessed_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        entry
    }

    /// 搜索已归档记忆（"努力回忆"）
    pub async fn search_archived(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        let archived = store.get_archived_memories(limit)?;

        // 简单 LIKE 过滤
        let pattern = query.to_lowercase();
        let results: Vec<MemoryEntry> = archived
            .iter()
            .filter(|m| m.content.to_lowercase().contains(&pattern))
            .map(Self::memory_to_entry)
            .collect();

        Ok(results)
    }

    /// 获取已归档记忆数量
    pub async fn archived_count(&self) -> Result<usize> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        store.count_archived()
    }

    /// 衰减所有未归档记忆的强度
    pub async fn decay_strength(&self) -> Result<usize> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        store.decay_strength(0.999)
    }

    /// 按事件类型查询记忆（排除已归档）
    pub fn get_by_event_type(&self, event_type: &str, offset: usize, limit: usize) -> Result<Vec<MemoryEntry>> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        let memories = store.get_memories_by_type(event_type, offset, limit)?;
        Ok(memories.iter().map(Self::memory_to_entry).collect())
    }
}

#[async_trait]
impl MemoryBackend for EpisodicMemoryBackend {
    fn name(&self) -> &'static str {
        "EpisodicMemory"
    }

    async fn add(&mut self, memory: &mut MemoryEntry) -> Result<i64> {
        // 只保存高重要性的记忆
        if memory.importance_score < self.save_threshold {
            return Ok(-1);
        }

        let mut client_memory = Self::entry_to_memory(memory);
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        let id = store.add_memory(&client_memory)?;
        client_memory.id = Some(id);
        // 回填 memory.id
        memory.id = Some(id);
        Ok(id)
    }

    async fn count(&self) -> Result<usize> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        store.count()
    }

    async fn clear(&mut self) -> Result<()> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        store.clear_all()
    }
}

#[async_trait]
impl SearchableBackend for EpisodicMemoryBackend {
    async fn get_top_by_importance(&self, limit: usize) -> Result<Vec<MemoryEntry>> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        let memories = store.get_top_memories(limit)?;
        Ok(memories.iter().map(Self::memory_to_entry).collect())
    }

    async fn get_recent(&self, limit: usize) -> Result<Vec<MemoryEntry>> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        let memories = store.get_recent_memories(limit)?;
        Ok(memories.iter().map(Self::memory_to_entry).collect())
    }

    async fn get_by_tick_range(&self, start: i64, end: i64) -> Result<Vec<MemoryEntry>> {
        // 使用 get_recent_memories 并过滤
        // 注意：这是一个简化实现，生产环境应该有专门的查询
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        let memories = store.get_recent_memories(1000)?;
        let filtered: Vec<MemoryEntry> = memories
            .iter()
            .filter(|m| m.tick_id >= start && m.tick_id <= end)
            .map(Self::memory_to_entry)
            .collect();
        Ok(filtered)
    }
}

#[async_trait]
impl ForgettableBackend for EpisodicMemoryBackend {
    async fn compute_forgotten(&self, threshold: f32) -> Result<Vec<MemoryEntry>> {
        // 获取所有记忆，过滤出保留率低于阈值的
        // 这是一个简化实现，实际应该使用 ForgettingScheduler
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        let memories = store.get_recent_memories(1000)?;
        let forgotten: Vec<MemoryEntry> = memories
            .iter()
            .filter(|m| m.importance_score < threshold)
            .map(Self::memory_to_entry)
            .collect();
        Ok(forgotten)
    }

    async fn archive_memories(&mut self, ids: &[i64]) -> Result<usize> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        store.archive_by_ids(ids)
    }

    async fn strengthen_memory(&mut self, id: i64) -> Result<()> {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow::anyhow!("Lock poisoned"))?;
        store.update_strength(id)
    }
}

// 注意：EpisodicMemoryBackend 不能实现 Default，因为它需要 agent_id 和 db_dir

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_backend() -> (EpisodicMemoryBackend, Uuid, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let agent_id = Uuid::new_v4();
        let backend = EpisodicMemoryBackend::new(agent_id, temp_dir.path()).unwrap();
        (backend, agent_id, temp_dir)
    }

    fn create_test_entry(agent_id: Uuid, importance: f32) -> MemoryEntry {
        MemoryEntry::new(agent_id, 1, "测试记忆".to_string()).with_importance(importance)
    }

    #[tokio::test]
    async fn test_add_filters_low_importance() {
        let (mut backend, agent_id, _temp) = create_test_backend();

        // 添加低重要性记忆（应该被过滤）
        let mut entry1 = create_test_entry(agent_id, 0.3);
        backend.add(&mut entry1).await.unwrap();
        assert_eq!(backend.count().await.unwrap(), 0);

        // 添加高重要性记忆（应该被保存）
        let mut entry2 = create_test_entry(agent_id, 0.7);
        backend.add(&mut entry2).await.unwrap();
        assert_eq!(backend.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_get_top_by_importance() {
        let (mut backend, agent_id, _temp) = create_test_backend();

        // 添加多个不同重要性的记忆
        for i in 1..=5 {
            let mut entry = MemoryEntry::new(agent_id, i, format!("记忆 {}", i))
                .with_importance(0.5 + i as f32 * 0.1);
            backend.add(&mut entry).await.unwrap();
        }

        let top = backend.get_top_by_importance(3).await.unwrap();
        assert_eq!(top.len(), 3);
        // 重要性应该递减
        assert!(top[0].importance_score >= top[1].importance_score);
    }

    #[tokio::test]
    async fn test_get_recent() {
        let (mut backend, agent_id, _temp) = create_test_backend();

        for i in 1..=5 {
            let mut entry =
                MemoryEntry::new(agent_id, i, format!("记忆 {}", i)).with_importance(0.6);
            backend.add(&mut entry).await.unwrap();
        }

        let recent = backend.get_recent(3).await.unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[tokio::test]
    async fn test_count_and_clear() {
        let (mut backend, agent_id, _temp) = create_test_backend();

        for _ in 1..=3 {
            let mut entry = create_test_entry(agent_id, 0.6);
            backend.add(&mut entry).await.unwrap();
        }

        assert_eq!(backend.count().await.unwrap(), 3);

        backend.clear().await.unwrap();
        assert_eq!(backend.count().await.unwrap(), 0);
    }
}
