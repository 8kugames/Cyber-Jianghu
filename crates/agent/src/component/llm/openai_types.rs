// ============================================================================
// OpenAI 兼容 API 类型定义
// ============================================================================
//
// 从 direct_client.rs 提取的 HTTP 通信类型。
// 支持 /v1/chat/completions 接口的工具调用功能。

use serde::{Deserialize, Serialize};

/// OpenAI 兼容 API 请求
#[derive(Debug, Clone, Serialize)]
pub(crate) struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// 工具定义（OpenAI function calling）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<super::tool_types::ToolDefinition>>,
    /// 工具选择策略（"auto" | "none" | specific）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    /// DashScope/Kimi 要求非流式调用禁用 thinking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
    /// 启用 SSE 流式响应
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// 流式响应选项（用于请求包含 usage 数据）
    /// 设置 `{"include_usage": true}` 可在流式响应的最后一块获取 token 用量
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<serde_json::Value>,
}

/// 聊天消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// assistant 消息中的 tool_calls
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<super::tool_types::ToolCall>>,
    /// tool 结果消息的 tool_call_id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// tool 结果消息的 name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub(crate) fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub(crate) fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub(crate) fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    pub(crate) fn tool_result(tool_call_id: &str, name: &str, content: &str) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
            name: Some(name.to_string()),
        }
    }
}

/// OpenAI 兼容 API 响应
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIResponse {
    pub choices: Vec<OpenAIChoice>,
    #[serde(default)]
    pub usage: Option<OpenAIUsage>,
    #[serde(default)]
    pub model: Option<String>,
}

/// Token 用量
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
}

/// 响应选项
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIChoice {
    pub message: ChatMessage,
}

// ============================================================================
// SSE 流式响应类型
// ============================================================================

/// SSE 流式响应（每个 chunk）
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIStreamResponse {
    pub choices: Vec<OpenAIStreamChoice>,
    #[serde(default)]
    pub usage: Option<OpenAIUsage>,
}

/// 流式响应选项
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIStreamChoice {
    pub delta: OpenAIDelta,
    #[serde(default)]
    #[allow(dead_code)]
    pub finish_reason: Option<String>,
}

/// 流式增量内容
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIDelta {
    #[serde(default)]
    pub content: Option<String>,
}
