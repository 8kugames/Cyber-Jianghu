// ============================================================================
// OpenClaw HTTP API LLM Client
// ============================================================================
//
// 通过 OpenClaw Gateway 的 HTTP API 调用 LLM
//
// OpenAI 兼容端点:
//   POST /v1/chat/completions
//   Authorization: Bearer YOUR_TOKEN
// ============================================================================

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use super::LlmClient;

/// OpenClaw HTTP API LLM 客户端
///
/// 通过 OpenClaw Gateway 的 OpenAI 兼容 API 调用已配置的模型
#[derive(Clone, Debug)]
pub struct OpenClawLLMClient {
    /// OpenClaw Gateway 地址 (如 http://localhost:23333)
    gateway_url: String,
    /// 认证 Token
    auth_token: String,
    /// 模型名称 (如 claude-opus-4-6, gpt-4)
    model: String,
    /// 温度参数 (0.0 - 1.0)
    temperature: f32,
    /// 最大 tokens
    max_tokens: u32,
}

impl OpenClawLLMClient {
    /// 创建新的 OpenClaw LLM 客户端
    ///
    /// # 参数
    ///
    /// - `gateway_url`: OpenClaw Gateway 地址
    /// - `auth_token`: 认证 Token
    /// - `model`: 模型名称
    pub fn new(
        gateway_url: impl Into<String>,
        auth_token: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            gateway_url: gateway_url.into(),
            auth_token: auth_token.into(),
            model: model.into(),
            temperature: 0.7,
            max_tokens: 4096,
        }
    }

    /// 设置温度参数
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature.clamp(0.0, 1.0);
        self
    }

    /// 设置最大 tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// 构建 HTTP 客户端
    fn build_http_client(&self) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("Failed to build HTTP client")
    }

    /// 获取聊天完成端点 URL
    fn chat_completions_url(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.gateway_url.trim_end_matches('/')
        )
    }

    /// 执行 HTTP POST 请求
    async fn post_chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        let client = self.build_http_client()?;
        let url = self.chat_completions_url();

        debug!("Calling OpenClaw LLM API: {}", url);
        debug!("Request model: {}", self.model);

        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.auth_token))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to OpenClaw")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            error!("OpenClaw API error {}: {}", status, error_body);
            anyhow::bail!("OpenClaw API error {}: {}", status, error_body);
        }

        response
            .json::<ChatCompletionResponse>()
            .await
            .context("Failed to parse OpenClaw response")
    }
}

#[async_trait]
impl LlmClient for OpenClawLLMClient {
    /// 完成一次 LLM 调用
    async fn complete(&self, prompt: &str) -> Result<String> {
        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            temperature: Some(self.temperature),
            max_tokens: Some(self.max_tokens),
        };

        let response = self.post_chat_completion(request).await?;

        if let Some(choice) = response.choices.first() {
            let content = choice.message.content.trim().to_string();
            debug!("LLM response length: {} chars", content.len());
            Ok(content)
        } else {
            anyhow::bail!("OpenClaw returned empty response")
        }
    }
}

// ============================================================================
// OpenAI 兼容 API 类型定义
// ============================================================================

/// 聊天完成请求
#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

/// 聊天消息
#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

/// 聊天完成响应
///
/// 字段用于 API 反序列化，部分字段预留给未来的日志和成本追踪功能
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChatCompletionResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<ChatChoice>,
    usage: ChatUsage,
}

/// 聊天选择
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChatChoice {
    index: u32,
    message: ChatMessage,
    finish_reason: Option<String>,
}

/// Token 使用情况（预留：成本追踪）
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChatUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_completions_url() {
        let client = OpenClawLLMClient::new("http://localhost:23333", "test-token", "gpt-4");
        assert_eq!(
            client.chat_completions_url(),
            "http://localhost:23333/v1/chat/completions"
        );

        let client = OpenClawLLMClient::new("http://localhost:23333/", "test-token", "gpt-4");
        assert_eq!(
            client.chat_completions_url(),
            "http://localhost:23333/v1/chat/completions"
        );
    }

    #[test]
    fn test_with_temperature_clamping() {
        let client = OpenClawLLMClient::new("http://localhost:23333", "test-token", "gpt-4")
            .with_temperature(-0.5);

        // 温度应该被限制在 [0.0, 1.0]
        assert_eq!(client.temperature, 0.0);

        let client = OpenClawLLMClient::new("http://localhost:23333", "test-token", "gpt-4")
            .with_temperature(1.5);
        assert_eq!(client.temperature, 1.0);
    }

    #[test]
    fn test_chat_completion_request_serialization() {
        let request = ChatCompletionRequest {
            model: "gpt-4".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
            temperature: Some(0.7),
            max_tokens: Some(1000),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"gpt-4\""));
        assert!(json.contains("\"temperature\":0.7"));
        assert!(json.contains("\"max_tokens\":1000"));
    }
}
