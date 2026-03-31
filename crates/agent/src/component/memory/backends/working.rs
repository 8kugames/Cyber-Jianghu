// ============================================================================
// 工作记忆后端（RAM FIFO）
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 保留最近 N 条事件，用于 LLM 上下文构建
// 使用 FIFO（先进先出）淘汰机制
// ============================================================================

use crate::component::memory::backend::{MemoryBackend, SearchableBackend};
use crate::component::memory::types::MemoryEntry;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::VecDeque;

/// 工作记忆后端
///
/// RAM 中的 FIFO 队列，存储最近 N 条事件
/// 用于 LLM 上下文构建
pub struct WorkingMemoryBackend {
    /// 事件队列（最新的在前面）
    events: VecDeque<MemoryEntry>,
    /// 最大事件数量
    max_size: usize,
}

impl WorkingMemoryBackend {
    /// 创建新的工作记忆后端
    pub fn new(max_size: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    /// 使用默认大小（20）创建
    pub fn with_default_size() -> Self {
        Self::new(20)
    }

    /// 获取前 N 条事件
    pub fn get_top_n(&self, n: usize) -> Vec<&MemoryEntry> {
        self.events.iter().take(n).collect()
    }

    /// 清空工作记忆
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// 构建 LLM 上下文（格式化为自然语言）
    pub fn build_context(&self) -> String {
        if self.events.is_empty() {
            return "暂无近期记忆".to_string();
        }

        self.events
            .iter()
            .enumerate()
            .map(|(i, event)| format!("{}. [Tick {}] {}", i + 1, event.tick_id, event.content))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 按重要性过滤事件
    pub fn filter_by_importance(&self, min_importance: f32) -> Vec<&MemoryEntry> {
        self.events
            .iter()
            .filter(|e| e.importance_score >= min_importance)
            .collect()
    }

    /// 获取所有事件的克隆
    pub fn to_vec(&self) -> Vec<MemoryEntry> {
        self.events.iter().cloned().collect()
    }
}

#[async_trait]
impl MemoryBackend for WorkingMemoryBackend {
    fn name(&self) -> &'static str {
        "WorkingMemory"
    }

    async fn add(&mut self, memory: MemoryEntry) -> Result<()> {
        if self.events.len() == self.max_size {
            self.events.pop_back(); // 移除最旧的事件
        }
        self.events.push_front(memory);
        Ok(())
    }

    async fn count(&self) -> Result<usize> {
        Ok(self.events.len())
    }

    async fn clear(&mut self) -> Result<()> {
        self.events.clear();
        Ok(())
    }
}

#[async_trait]
impl SearchableBackend for WorkingMemoryBackend {
    async fn get_top_by_importance(&self, limit: usize) -> Result<Vec<MemoryEntry>> {
        let mut events: Vec<_> = self.events.iter().cloned().collect();
        events.sort_by(|a, b| {
            b.importance_score
                .partial_cmp(&a.importance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        events.truncate(limit);
        Ok(events)
    }

    async fn get_recent(&self, limit: usize) -> Result<Vec<MemoryEntry>> {
        Ok(self.events.iter().take(limit).cloned().collect())
    }

    async fn get_by_tick_range(&self, start: i64, end: i64) -> Result<Vec<MemoryEntry>> {
        Ok(self
            .events
            .iter()
            .filter(|e| e.tick_id >= start && e.tick_id <= end)
            .cloned()
            .collect())
    }
}

impl Default for WorkingMemoryBackend {
    fn default() -> Self {
        Self::with_default_size()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn create_test_memory(tick_id: i64, content: &str, importance: f32) -> MemoryEntry {
        MemoryEntry::new(Uuid::nil(), tick_id, content.to_string()).with_importance(importance)
    }

    #[tokio::test]
    async fn test_fifo_eviction() {
        let mut backend = WorkingMemoryBackend::new(3);

        backend
            .add(create_test_memory(1, "Event A", 0.5))
            .await
            .unwrap();
        backend
            .add(create_test_memory(2, "Event B", 0.5))
            .await
            .unwrap();
        backend
            .add(create_test_memory(3, "Event C", 0.5))
            .await
            .unwrap();
        assert_eq!(backend.count().await.unwrap(), 3);

        // 添加第 4 个事件，应该淘汰 Event A
        backend
            .add(create_test_memory(4, "Event D", 0.5))
            .await
            .unwrap();
        assert_eq!(backend.count().await.unwrap(), 3);

        let events = backend.get_recent(10).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].content, "Event D");
        assert_eq!(events[2].content, "Event B");
    }

    #[tokio::test]
    async fn test_get_top_by_importance() {
        let mut backend = WorkingMemoryBackend::new(10);

        backend
            .add(create_test_memory(1, "Low", 0.2))
            .await
            .unwrap();
        backend
            .add(create_test_memory(2, "High", 0.9))
            .await
            .unwrap();
        backend
            .add(create_test_memory(3, "Medium", 0.5))
            .await
            .unwrap();

        let top = backend.get_top_by_importance(2).await.unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].content, "High");
        assert_eq!(top[1].content, "Medium");
    }

    #[tokio::test]
    async fn test_get_by_tick_range() {
        let mut backend = WorkingMemoryBackend::new(10);

        backend
            .add(create_test_memory(1, "Event 1", 0.5))
            .await
            .unwrap();
        backend
            .add(create_test_memory(5, "Event 5", 0.5))
            .await
            .unwrap();
        backend
            .add(create_test_memory(10, "Event 10", 0.5))
            .await
            .unwrap();

        let events = backend.get_by_tick_range(3, 10).await.unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_build_context() {
        let mut backend = WorkingMemoryBackend::new(10);
        backend
            .events
            .push_front(create_test_memory(1, "你吃了馒头", 0.3));
        backend
            .events
            .push_front(create_test_memory(2, "你喝了水", 0.3));

        let context = backend.build_context();
        assert!(context.contains("你吃了馒头"));
        assert!(context.contains("你喝了水"));
    }
}
