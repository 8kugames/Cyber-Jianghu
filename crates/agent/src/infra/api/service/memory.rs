// ============================================================================
// Memory Service - 记忆业务逻辑
// ============================================================================
//
// 从 handlers.rs 提取的记忆相关业务逻辑

use anyhow::Result;
use tracing::info;
use uuid::Uuid;

use crate::component::memory::backend::MemoryBackend;
use crate::component::memory::{MemoryEntry, MemoryManager};

/// 记忆服务
pub struct MemoryService<'a> {
    manager: &'a mut MemoryManager,
}

impl<'a> MemoryService<'a> {
    /// 创建新的记忆服务实例
    pub fn new(manager: &'a mut MemoryManager) -> Self {
        Self { manager }
    }

    /// 获取近期记忆（工作记忆）
    pub fn get_recent(&self) -> Vec<MemoryEntry> {
        self.manager.working().to_vec()
    }

    /// 搜索归档记忆
    pub async fn search(&mut self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        self.manager.recall_archived(query, limit).await
    }

    /// 存储记忆
    pub async fn store(
        &mut self,
        agent_id: Uuid,
        tick_id: i64,
        content: String,
        importance: Option<f32>,
    ) -> Result<()> {
        let mut entry = MemoryEntry::new(agent_id, tick_id, content.clone());

        if let Some(imp) = importance {
            entry = entry.with_importance(imp);
        }

        self.manager.working_mut().add(&mut entry).await?;

        info!("[memory] Memory stored: {}", content);

        Ok(())
    }

    /// 获取每日摘要记忆
    /// 返回 (数据列表, 是否还有更多)
    /// has_more 采用 limit+1 技巧：请求 limit+1 条，若返回 > limit 条则说明还有更多
    pub fn get_daily_summaries(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<MemoryEntry>, bool)> {
        let memories =
            self.manager
                .episodic()
                .get_by_event_type("daily_summary", offset, limit + 1)?;
        let has_more = memories.len() > limit;
        let memories = memories.into_iter().take(limit).collect();
        Ok((memories, has_more))
    }
}

// ============================================================================
// 记忆格式化工具
// ============================================================================

/// 单条记忆转换为 JSON
pub fn memory_to_json(memory: &MemoryEntry) -> serde_json::Value {
    serde_json::json!({
        "tick_id": memory.tick_id,
        "content": memory.content,
        "importance": memory.importance_score,
        "created_at": memory.created_at,
    })
}

/// 记忆列表转换为 JSON 响应
pub fn memories_to_json_response(memories: &[MemoryEntry]) -> serde_json::Value {
    let results: Vec<serde_json::Value> = memories.iter().map(memory_to_json).collect();
    serde_json::json!({
        "memories": results,
        "count": results.len(),
        "has_more": false,
    })
}

/// 搜索结果转换为 JSON 响应
pub fn search_result_to_json(memories: &[MemoryEntry], query: &str) -> serde_json::Value {
    let results: Vec<serde_json::Value> = memories.iter().map(memory_to_json).collect();
    serde_json::json!({
        "memories": results,
        "count": results.len(),
        "query": query,
        "has_more": false,
    })
}
