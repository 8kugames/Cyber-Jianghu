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

    /// 完成一次 LLM 调用（system + user 分离）
    ///
    /// 使用 system role 发送系统指令，user role 发送用户 prompt，
    /// 利用 LLM 的 system message 优先级机制确保角色指令不被截断。
    async fn complete_with_system(&self, system: &str, prompt: &str) -> Result<String>;
}

/// LlmClient 扩展 Trait
///
/// 提供 complete_json 等辅助方法
#[async_trait]
pub trait LlmClientExt {
    /// 完成一次结构化输出调用（JSON 模式）
    async fn complete_json<T: DeserializeOwned + Send>(&self, prompt: &str) -> Result<T>;

    /// 完成一次结构化输出调用（JSON 模式，system + user 分离）
    async fn complete_json_with_system<T: DeserializeOwned + Send>(
        &self,
        system: &str,
        prompt: &str,
    ) -> Result<T>;
}

/// 从 LLM 响应中提取 JSON 字符串
fn extract_json_str(response: &str) -> &str {
    if let Some(start) = response.find("```json") {
        let after_marker = start + 7;
        if let Some(end) = response[after_marker..].find("```") {
            response[after_marker..after_marker + end].trim()
        } else {
            response[after_marker..].trim()
        }
    } else if let Some(start) = response.find('{') {
        if let Some(end) = response.rfind('}') {
            &response[start..=end]
        } else {
            response.trim()
        }
    } else {
        response.trim()
    }
}

/// 尝试修复截断的 JSON 字符串
fn try_fix_truncated_json(json_str: &str) -> String {
    let mut fixed = json_str.trim_end().to_string();

    // 移除尾部逗号
    if fixed.ends_with(',') {
        fixed.pop();
    }

    // 修复截断的字符串值（未闭合的引号）
    // 检查最后一个 `"` 之后是否有奇数个引号，如果有则补一个
    let quote_count = fixed.chars().filter(|&c| c == '"').count();
    if quote_count % 2 != 0 {
        fixed.push('"');
    }

    // 闭合未关闭的括号
    let open_brackets = fixed.chars().filter(|&c| c == '[').count() as i32;
    let close_brackets = fixed.chars().filter(|&c| c == ']').count() as i32;
    let open_braces = fixed.chars().filter(|&c| c == '{').count() as i32;
    let close_braces = fixed.chars().filter(|&c| c == '}').count() as i32;

    for _ in 0..(open_brackets - close_brackets).max(0) {
        fixed.push(']');
    }
    for _ in 0..(open_braces - close_braces).max(0) {
        fixed.push('}');
    }

    fixed
}

/// 解析 LLM 响应为结构化类型（带截断修复）
fn parse_json_response<D: DeserializeOwned + Send>(response: &str) -> Result<D> {
    let json_str = extract_json_str(response);

    match serde_json::from_str::<D>(json_str) {
        Ok(parsed) => Ok(parsed),
        Err(first_err) => {
            let fixed = try_fix_truncated_json(json_str);

            match serde_json::from_str::<D>(&fixed) {
                Ok(parsed) => {
                    tracing::warn!("JSON was truncated, auto-fixed");
                    Ok(parsed)
                }
                Err(e) => {
                    tracing::error!(
                        "JSON parse failed even after fix attempt: {}. Raw response:\n{}",
                        e,
                        response
                    );
                    Err(first_err.into())
                }
            }
        }
    }
}

#[async_trait]
impl<T: LlmClient + ?Sized> LlmClientExt for T {
    async fn complete_json<D: DeserializeOwned + Send>(&self, prompt: &str) -> Result<D> {
        let response = self.complete(prompt).await?;
        parse_json_response::<D>(&response)
    }

    async fn complete_json_with_system<D: DeserializeOwned + Send>(
        &self,
        system: &str,
        prompt: &str,
    ) -> Result<D> {
        let response = self.complete_with_system(system, prompt).await?;
        parse_json_response::<D>(&response)
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

        async fn complete_with_system(&self, _system: &str, _prompt: &str) -> Result<String> {
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
