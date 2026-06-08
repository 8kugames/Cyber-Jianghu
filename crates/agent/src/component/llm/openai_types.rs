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
    /// 存储为 `serde_json::Value` 而非 `ToolDefinition`, 便于在 send_chat_exchange 中
    /// 先调 canonicalize_json_schema 规范化 (sort keys + required 数组) 再序列化,
    /// 保证 DeepSeek 前缀缓存的 tools 字段字节级稳定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<serde_json::Value>>,
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
pub struct ChatMessage {
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
    /// DeepSeek/SenseNova 等模型的思考内容，多轮对话必须回传
    #[serde(skip_serializing_if = "Option::is_none", alias = "reasoning")]
    pub reasoning_content: Option<String>,
}

impl ChatMessage {
    pub(crate) fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    pub(crate) fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    pub(crate) fn assistant_with_reasoning(content: &str, reasoning: Option<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: reasoning,
        }
    }

    pub(crate) fn tool_result(tool_call_id: &str, name: &str, content: &str) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
            name: Some(name.to_string()),
            reasoning_content: None,
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

/// Token 用量明细（嵌套格式，MiniMax / DeepSeek / OpenAI 兼容）
#[derive(Debug, Deserialize, Default)]
pub(crate) struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

impl OpenAIUsage {
    pub fn cache_hit_tokens(&self) -> Option<u64> {
        self.prompt_tokens_details.as_ref()?.cached_tokens
    }
}

/// 响应选项
#[derive(Debug, Deserialize)]
pub(crate) struct OpenAIChoice {
    pub message: ChatMessage,
}

// ============================================================================
// LLM 消息交换抽象（mode-agnostic）
// ============================================================================

/// LLM 原始消息交换的统一响应
///
/// 与 LLM 接入方式（HTTP / WebSocket）无关的响应类型，
/// 由 `send_chat_exchange` trait 方法返回。
pub struct ChatExchangeResponse {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<super::tool_types::ToolCall>>,
    pub reasoning_content: Option<String>,
}

/// LLM 消息交换的调用参数
#[derive(Clone)]
pub struct ChatExchangeConfig {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub enable_thinking: Option<bool>,
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
    /// 流式 tool_calls 增量（首个 chunk 含 id/name，后续 chunk 含 arguments 片段）
    #[serde(default)]
    pub tool_calls: Option<Vec<super::tool_types::StreamToolCallDelta>>,
    /// 推理/思考内容（SenseNova、DeepSeek 等模型在 thinking 模式下返回）
    /// 这些 token 计入 completion_tokens 但不进入 content 字段
    #[serde(default, alias = "reasoning")]
    pub reasoning_content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_delta_with_tool_calls() {
        let json = r#"{"role":"assistant","content":"","tool_calls":[{"id":"call_abc","index":0,"type":"function","function":{"name":"skill_view","arguments":""}}]}"#;
        let delta: OpenAIDelta = serde_json::from_str(json).unwrap();
        assert!(delta.tool_calls.is_some(), "tool_calls should be present");
        let tc = delta.tool_calls.unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id.as_deref(), Some("call_abc"));
        assert_eq!(tc[0].index, 0);
        assert_eq!(tc[0].function.name, "skill_view");
    }

    #[test]
    fn test_openai_stream_response_with_tool_calls() {
        let json = r#"{"id":"test","object":"chat.completion.chunk","created":1,"model":"test","choices":[{"index":0,"delta":{"tool_calls":[{"id":"call_abc","index":0,"type":"function","function":{"name":"skill_view","arguments":""}}]},"finish_reason":null}]}"#;
        let resp: OpenAIStreamResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.choices.len(), 1);
        let delta = &resp.choices[0].delta;
        assert!(
            delta.tool_calls.is_some(),
            "tool_calls should be present in stream response"
        );
        let tc = delta.tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].function.name, "skill_view");
    }

    #[test]
    fn test_openai_delta_content_only() {
        let json = r#"{"role":"assistant","content":"hello"}"#;
        let delta: OpenAIDelta = serde_json::from_str(json).unwrap();
        assert_eq!(delta.content.as_deref(), Some("hello"));
        assert!(delta.tool_calls.is_none());
    }
}
