// ============================================================================
// 记忆后端 Trait 定义
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md

use crate::component::memory::types::MemoryEntry;
use anyhow::Result;
use async_trait::async_trait;

/// 记忆后端基础 Trait
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// 后端名称（用于日志和调试）
    fn name(&self) -> &'static str;

    /// 添加单条记忆
    async fn add(&mut self, memory: MemoryEntry) -> Result<()>;

    /// 批量添加记忆
    async fn add_batch(&mut self, memories: Vec<MemoryEntry>) -> Result<usize> {
        let mut count = 0;
        for memory in memories {
            self.add(memory).await?;
            count += 1;
        }
        Ok(count)
    }

    /// 获取记忆数量
    async fn count(&self) -> Result<usize>;

    /// 清空所有记忆
    async fn clear(&mut self) -> Result<()>;
}

/// 可检索的记忆后端
#[async_trait]
pub trait SearchableBackend: MemoryBackend {
    /// 按重要性获取 Top K
    async fn get_top_by_importance(&self, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// 获取最近的 N 条记忆
    async fn get_recent(&self, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// 按 tick 范围获取记忆
    async fn get_by_tick_range(&self, start: i64, end: i64) -> Result<Vec<MemoryEntry>>;
}

/// 语义检索后端
#[async_trait]
pub trait SemanticSearchable: SearchableBackend {
    /// 语义相似度检索
    async fn search_similar(&mut self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// 为记忆生成嵌入向量（按需）
    async fn ensure_embedding(&mut self, memory_id: i64) -> Result<()>;
}

/// 可遗忘的记忆后端
#[async_trait]
pub trait ForgettableBackend: SearchableBackend {
    /// 执行遗忘计算，返回需要降级的记忆
    async fn compute_forgotten(&self, threshold: f32) -> Result<Vec<MemoryEntry>>;

    /// 移动记忆到归档
    async fn archive_memories(&mut self, ids: &[i64]) -> Result<usize>;

    /// 更新记忆强度（检索增强）
    async fn strengthen_memory(&mut self, id: i64) -> Result<()>;
}
