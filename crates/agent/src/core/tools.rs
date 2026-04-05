use crate::component::memory::MemoryManager;
use crate::component::memory::backend::SearchableBackend;
use crate::component::memory::tools::{
    MemorySearchResult, MemoryToolDefinition, MemoryToolResult, RECALL_ARCHIVED_TOOL,
    RecallArchivedParams, SEARCH_MEMORY_TOOL, SearchMemoryParams,
};

/// 获取所有记忆工具定义（供 LLM function calling）
pub fn get_memory_tools() -> Vec<MemoryToolDefinition> {
    MemoryToolDefinition::all()
}

/// 执行工具调用
pub async fn execute_tool_call(
    memory_manager: &mut Option<MemoryManager>,
    _world_state: &crate::models::WorldState,
    tool_name: &str,
    arguments: &str,
) -> MemoryToolResult {
    let manager = match memory_manager {
        Some(m) => m,
        None => return MemoryToolResult::error("Memory manager not initialized"),
    };

    match tool_name {
        SEARCH_MEMORY_TOOL => {
            let params: SearchMemoryParams = match serde_json::from_str(arguments) {
                Ok(p) => p,
                Err(e) => return MemoryToolResult::error(format!("Invalid parameters: {}", e)),
            };

            match manager.episodic().get_recent(params.limit).await {
                Ok(memories) => {
                    let results: Vec<MemorySearchResult> = memories
                        .into_iter()
                        .map(|m| MemorySearchResult {
                            content: m.content,
                            tick_id: m.tick_id,
                            importance_score: m.importance_score,
                            source: "episodic".to_string(),
                        })
                        .collect();

                    if results.is_empty() {
                        MemoryToolResult::empty()
                    } else {
                        MemoryToolResult::success(results)
                    }
                }
                Err(e) => MemoryToolResult::error(format!("Search failed: {}", e)),
            }
        }

        RECALL_ARCHIVED_TOOL => {
            let params: RecallArchivedParams = match serde_json::from_str(arguments) {
                Ok(p) => p,
                Err(e) => return MemoryToolResult::error(format!("Invalid parameters: {}", e)),
            };

            match manager.recall_archived(&params.query, params.limit).await {
                Ok(memories) => {
                    let results: Vec<MemorySearchResult> = memories
                        .into_iter()
                        .map(|m| MemorySearchResult {
                            content: m.content,
                            tick_id: m.tick_id,
                            importance_score: m.importance_score,
                            source: "archive".to_string(),
                        })
                        .collect();

                    if results.is_empty() {
                        MemoryToolResult::empty()
                    } else {
                        MemoryToolResult::success(results)
                    }
                }
                Err(e) => MemoryToolResult::error(format!("Recall failed: {}", e)),
            }
        }

        _ => MemoryToolResult::error(format!("Unknown tool: {}", tool_name)),
    }
}
