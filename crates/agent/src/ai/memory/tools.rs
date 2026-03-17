// ============================================================================
// 记忆工具定义
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 提供供 LLM 调用的记忆检索工具定义
// - search_memory: 语义检索相关记忆
// - recall_archived: 努力回忆已遗忘的记忆
// ============================================================================

use serde::{Deserialize, Serialize};

/// 记忆工具名称
pub const SEARCH_MEMORY_TOOL: &str = "search_memory";
pub const RECALL_ARCHIVED_TOOL: &str = "recall_archived";

/// 记忆工具定义（供 LLM function calling 使用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryToolDefinition {
    /// 工具名称
    pub name: String,
    /// 工具描述
    pub description: String,
    /// 参数定义
    pub parameters: MemoryToolParameters,
}

/// 记忆工具参数定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryToolParameters {
    #[serde(rename = "type")]
    pub param_type: String,
    pub properties: serde_json::Value,
    pub required: Vec<String>,
}

/// search_memory 工具参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMemoryParams {
    /// 搜索查询描述
    pub query: String,
    /// 返回结果数量限制
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    5
}

/// recall_archived 工具参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallArchivedParams {
    /// 你想回忆的内容
    pub query: String,
    /// 返回结果数量限制
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// 记忆工具调用结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryToolResult {
    /// 是否成功
    pub success: bool,
    /// 结果消息
    pub message: String,
    /// 找到的记忆条目
    #[serde(default)]
    pub memories: Vec<MemorySearchResult>,
}

/// 单条记忆搜索结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchResult {
    /// 记忆内容
    pub content: String,
    /// 游戏时间 (tick)
    pub tick_id: i64,
    /// 重要性评分
    pub importance_score: f32,
    /// 来源类型
    pub source: String,
}

impl MemoryToolDefinition {
    /// 获取 search_memory 工具定义
    pub fn search_memory() -> Self {
        Self {
            name: SEARCH_MEMORY_TOOL.to_string(),
            description: "搜索相关记忆。当前情况让你想起过去的经历时使用。".to_string(),
            parameters: MemoryToolParameters {
                param_type: "object".to_string(),
                properties: serde_json::json!({
                    "query": {
                        "type": "string",
                        "description": "搜索查询描述"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "返回结果数量限制",
                        "default": 5
                    }
                }),
                required: vec!["query".to_string()],
            },
        }
    }

    /// 获取 recall_archived 工具定义
    pub fn recall_archived() -> Self {
        Self {
            name: RECALL_ARCHIVED_TOOL.to_string(),
            description: "努力回忆已模糊的记忆。用于回忆很久以前的事情。".to_string(),
            parameters: MemoryToolParameters {
                param_type: "object".to_string(),
                properties: serde_json::json!({
                    "query": {
                        "type": "string",
                        "description": "你想回忆的内容"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "返回结果数量限制",
                        "default": 5
                    }
                }),
                required: vec!["query".to_string()],
            },
        }
    }

    /// 获取所有记忆工具定义
    pub fn all() -> Vec<Self> {
        vec![Self::search_memory(), Self::recall_archived()]
    }

    /// 转换为 OpenAI/Anthropic 兼容的 JSON 格式
    pub fn to_openai_format(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": {
                    "type": self.parameters.param_type,
                    "properties": self.parameters.properties,
                    "required": self.parameters.required
                }
            }
        })
    }
}

impl MemorySearchResult {
    /// 从 MemoryEntry 创建搜索结果
    pub fn from_entry(entry: &crate::ai::memory::types::MemoryEntry, source: &str) -> Self {
        Self {
            content: entry.content.clone(),
            tick_id: entry.tick_id,
            importance_score: entry.importance_score,
            source: source.to_string(),
        }
    }
}

impl MemoryToolResult {
    /// 创建成功结果
    pub fn success(memories: Vec<MemorySearchResult>) -> Self {
        let count = memories.len();
        Self {
            success: true,
            message: format!("找到 {} 条相关记忆", count),
            memories,
        }
    }

    /// 创建空结果
    pub fn empty() -> Self {
        Self {
            success: true,
            message: "没有找到相关记忆".to_string(),
            memories: Vec::new(),
        }
    }

    /// 创建错误结果
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            memories: Vec::new(),
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions() {
        let tools = MemoryToolDefinition::all();
        assert_eq!(tools.len(), 2);

        let search = MemoryToolDefinition::search_memory();
        assert_eq!(search.name, "search_memory");
        assert!(search.parameters.required.contains(&"query".to_string()));

        let recall = MemoryToolDefinition::recall_archived();
        assert_eq!(recall.name, "recall_archived");
    }

    #[test]
    fn test_openai_format() {
        let tool = MemoryToolDefinition::search_memory();
        let json = tool.to_openai_format();

        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "search_memory");
        assert!(json["function"]["parameters"]["properties"]["query"].is_object());
    }

    #[test]
    fn test_search_params_deserialization() {
        let json = r#"{"query": "战斗", "limit": 10}"#;
        let params: SearchMemoryParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.query, "战斗");
        assert_eq!(params.limit, 10);
    }

    #[test]
    fn test_search_params_default_limit() {
        let json = r#"{"query": "test"}"#;
        let params: SearchMemoryParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.limit, 5);
    }

    #[test]
    fn test_memory_tool_result() {
        let result = MemoryToolResult::success(vec![
            MemorySearchResult {
                content: "测试记忆".to_string(),
                tick_id: 100,
                importance_score: 0.8,
                source: "episodic".to_string(),
            },
        ]);
        assert!(result.success);
        assert_eq!(result.memories.len(), 1);

        let empty = MemoryToolResult::empty();
        assert!(empty.success);
        assert!(empty.memories.is_empty());

        let error = MemoryToolResult::error("查询失败");
        assert!(!error.success);
    }
}
