//! Mock LLM 客户端（仅用于测试）

use super::*;
use std::sync::{Arc, Mutex};

/// Mock LLM 客户端（仅用于测试）
pub struct MockLlmClient {
    response: Arc<Mutex<String>>,
}

impl MockLlmClient {
    /// 创建带有预设响应的 Mock 客户端
    pub fn with_response(response: &str) -> Self {
        Self {
            response: Arc::new(Mutex::new(response.to_string())),
        }
    }

    /// 更新预设响应
    pub fn set_response(&self, response: &str) {
        *self.response.lock().expect("lock poisoned") = response.to_string();
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _prompt: &str) -> Result<String> {
        Ok(self.response.lock().expect("lock poisoned").clone())
    }

    async fn complete_with_system(&self, _system: &str, _prompt: &str) -> Result<String> {
        Ok(self.response.lock().expect("lock poisoned").clone())
    }

    async fn send_chat_exchange(
        &self,
        _messages: Vec<crate::component::llm::ChatMessage>,
        _tools: Option<&[crate::component::llm::ToolDefinition]>,
        _config: crate::component::llm::ChatExchangeConfig,
    ) -> Result<crate::component::llm::openai_types::ChatExchangeResponse> {
        Ok(crate::component::llm::openai_types::ChatExchangeResponse {
            content: Some(self.response.lock().expect("lock poisoned").clone()),
            tool_calls: None,
            reasoning_content: None,
        })
    }
}
