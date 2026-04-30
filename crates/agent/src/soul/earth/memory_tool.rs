// ============================================================================
// 记忆工具执行逻辑
// ============================================================================

use crate::component::llm::tool_types::ToolDefinition;
use crate::component::memory::manager::MemoryManager;

/// search_memory tool 定义
pub fn search_memory_definition() -> ToolDefinition {
    ToolDefinition::new(
        "search_memory",
        "搜索相关记忆。当前情况让你想起过去的经历时使用。",
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索查询描述"
                },
                "limit": {
                    "type": "integer",
                    "description": "返回结果数量限制",
                    "default": 5
                }
            },
            "required": ["query"]
        })),
    )
}

/// recall_archived tool 定义
pub fn recall_archived_definition() -> ToolDefinition {
    ToolDefinition::new(
        "recall_archived",
        "按时间倒序回忆近期被遗忘的事件。不需要提供查询词。",
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "你想回忆的内容"
                },
                "limit": {
                    "type": "integer",
                    "description": "返回结果数量限制",
                    "default": 5
                }
            },
            "required": []
        })),
    )
}

/// 执行 search_memory
pub(super) async fn execute_search_memory(
    memory_manager: &MemoryManager,
    query: &str,
    limit: usize,
) -> serde_json::Value {
    match memory_manager.recall_archived(query, limit).await {
        Ok(memories) => {
            if memories.is_empty() {
                return serde_json::json!({
                    "success": true,
                    "message": "没有找到相关记忆",
                    "memories": []
                });
            }
            let entries: Vec<serde_json::Value> = memories
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "content": m.content,
                        "tick_id": m.tick_id,
                        "importance": m.importance_score
                    })
                })
                .collect();
            serde_json::json!({
                "success": true,
                "message": format!("找到 {} 条相关记忆", entries.len()),
                "memories": entries
            })
        }
        Err(e) => serde_json::json!({
            "success": false,
            "error": format!("搜索记忆失败: {}", e)
        }),
    }
}

/// 执行 recall_archived（按时间倒序，跳过语义搜索）
pub(super) async fn execute_recall_archived(
    memory_manager: &MemoryManager,
    limit: usize,
) -> serde_json::Value {
    match memory_manager.recall_recent_archived(limit).await {
        Ok(memories) => {
            if memories.is_empty() {
                return serde_json::json!({
                    "success": true,
                    "message": "没有近期被遗忘的事件",
                    "memories": []
                });
            }
            let entries: Vec<serde_json::Value> = memories
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "content": m.content,
                        "tick_id": m.tick_id,
                        "importance": m.importance_score
                    })
                })
                .collect();
            serde_json::json!({
                "success": true,
                "message": format!("回忆起 {} 条近期被遗忘的事件", entries.len()),
                "memories": entries
            })
        }
        Err(e) => serde_json::json!({
            "success": false,
            "error": format!("回忆记忆失败: {}", e)
        }),
    }
}
