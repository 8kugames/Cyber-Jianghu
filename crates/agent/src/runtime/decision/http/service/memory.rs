// ============================================================================
// Memory Service - 记忆业务逻辑
// ============================================================================
//
// 从 handlers.rs 提取的记忆相关业务逻辑

use anyhow::Result;
use tracing::info;
use uuid::Uuid;

use crate::ai::memory::backend::{MemoryBackend, SearchableBackend};
use crate::ai::memory::{MemoryEntry, MemoryManager};

/// 记忆服务
pub struct MemoryService<'a> {
    manager: &'a mut MemoryManager,
}

impl<'a> MemoryService<'a> {
    /// 创建新的记忆服务实例
    pub fn new(manager: &'a mut MemoryManager) -> Self {
        Self { manager }
    }

    /// 获取近期记忆（合并工作记忆 + 情景记忆，支持分页）
    pub async fn get_recent(&mut self, page: usize, limit: usize) -> (Vec<MemoryEntry>, usize) {
        let working = self.manager.working().to_vec();

        let episodic = self
            .manager
            .episodic()
            .get_recent(1000)
            .await
            .unwrap_or_default();

        let mut seen = std::collections::HashSet::new();
        let mut all_memories: Vec<MemoryEntry> = Vec::new();

        for m in episodic {
            let key = (m.tick_id, m.content.clone());
            if seen.insert(key) {
                all_memories.push(m);
            }
        }

        for m in working {
            let key = (m.tick_id, m.content.clone());
            if seen.insert(key) {
                all_memories.push(m);
            }
        }

        all_memories.sort_by(|a, b| b.tick_id.cmp(&a.tick_id));

        let total = all_memories.len();
        let offset = (page.saturating_sub(1)) * limit;
        let paged: Vec<MemoryEntry> = all_memories.into_iter().skip(offset).take(limit).collect();

        (paged, total)
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

        self.manager.working_mut().add(entry).await?;

        info!("[memory] Memory stored: {}", content);

        Ok(())
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
pub fn memories_to_json_response(
    memories: &[MemoryEntry],
    total: usize,
    page: usize,
    limit: usize,
) -> serde_json::Value {
    let results: Vec<serde_json::Value> = memories.iter().map(memory_to_json).collect();
    let has_more = (page * limit) < total;
    serde_json::json!({
        "memories": results,
        "count": results.len(),
        "total": total,
        "has_more": has_more,
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
