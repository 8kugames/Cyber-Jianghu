// ============================================================================
// LLM 客户端接口
// ============================================================================
//
// 定义 LLM 客户端 Trait，仅由 OpenClaw 实现
// ============================================================================

use anyhow::Result;
use async_trait::async_trait;
use serde::de::DeserializeOwned;

/// LLM 客户端 Trait（仅由 OpenClaw 实现）
///
/// **重要约束**：
/// - 仅允许 OpenClaw 提供 LlmClient 实现
/// - SDK 不提供任何 LlmClient 的默认实现（Mock 除外，仅用于测试）
/// - 验证器和玩家 Agent 共享同一个 OpenClaw LlmClient 实例
/// - 所有 AI 调用（决策 + 验证 + 叙事）必须通过 OpenClaw
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// 完成一次 LLM 调用
    async fn complete(&self, prompt: &str) -> Result<String>;
}

/// 从 LLM 响应中提取第一个合法的 JSON object
///
/// LLM 可能在 JSON 前输出分析文字，简单的 find('{') + rfind('}')
/// 会跨越分析文字提取到无效 JSON。此函数逐个 '{' 尝试 brace-depth 匹配。
fn extract_first_json_object(input: &str) -> &str {
    for (i, ch) in input.char_indices() {
        if ch == '{' {
            // 从此位置开始 brace-depth 匹配
            let mut depth = 0i32;
            let mut in_string = false;
            let mut escape = false;
            let bytes_iter = input[i..].char_indices();

            for (offset, c) in bytes_iter {
                if escape {
                    escape = false;
                    continue;
                }
                match c {
                    '\\' if in_string => escape = true,
                    '"' => in_string = !in_string,
                    '{' if !in_string => depth += 1,
                    '}' if !in_string => {
                        depth -= 1;
                        if depth == 0 {
                            let candidate = &input[i..i + offset + c.len_utf8()];
                            // 快速验证是否合法 JSON
                            if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                                return candidate;
                            }
                            break; // depth 归零但不合法，跳到下一个 '{'
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // 全部失败，fallback 到 trim
    input.trim()
}

/// LlmClient 扩展 Trait
///
/// 提供 complete_json 等辅助方法
#[async_trait]
pub trait LlmClientExt {
    /// 完成一次结构化输出调用（JSON 模式）
    async fn complete_json<T: DeserializeOwned + Send>(&self, prompt: &str) -> Result<T>;
}

#[async_trait]
impl<T: LlmClient + ?Sized> LlmClientExt for T {
    async fn complete_json<D: DeserializeOwned + Send>(&self, prompt: &str) -> Result<D> {
        let response = self.complete(prompt).await?;

        // 优先从 markdown 代码块提取
        let json_str = if let Some(start) = response.find("```json") {
            let after_marker = start + 7;
            if let Some(end) = response[after_marker..].find("```") {
                response[after_marker..after_marker + end].trim()
            } else {
                response[after_marker..].trim()
            }
        } else {
            // 逐个 '{' 位置尝试 brace-depth 匹配，找第一个合法 JSON object
            extract_first_json_object(&response)
        };

        let parsed: D = serde_json::from_str(json_str).map_err(|e| {
            tracing::error!(
                "[complete_json] Failed to parse JSON: {}\nRaw JSON: {}",
                e,
                json_str
            );
            e
        })?;
        Ok(parsed)
    }
}

// ============================================================================
// Mock LLM 客户端（仅用于测试）
// ============================================================================

pub mod mock {
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
            *self.response.lock().unwrap() = response.to_string();
        }
    }

    #[async_trait]
    impl LlmClient for MockLlmClient {
        async fn complete(&self, _prompt: &str) -> Result<String> {
            Ok(self.response.lock().unwrap().clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::*;
    use super::*;

    #[tokio::test]
    async fn test_mock_llm_client_complete() {
        let client = MockLlmClient::with_response("Hello, world!");
        let result = client.complete("test prompt").await.unwrap();
        assert_eq!(result, "Hello, world!");
    }

    #[tokio::test]
    async fn test_mock_llm_client_complete_json() {
        #[derive(serde::Deserialize)]
        struct TestResponse {
            message: String,
        }

        let client = MockLlmClient::with_response(r#"{"message": "test"}"#);
        let result: TestResponse = client.complete_json("test prompt").await.unwrap();
        assert_eq!(result.message, "test");
    }
}
