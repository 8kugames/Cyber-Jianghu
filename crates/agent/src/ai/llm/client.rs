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

/// 从 LLM 响应中提取第一个 `{...}` 范围（不验证合法性）
///
/// 仅做 brace-depth 匹配，比 `extract_first_json_object` 更宽松：
/// 不要求内部引号完全合法，因为后续 `escape_unescaped_quotes` 会修复。
fn extract_json_span(input: &str) -> &str {
    for (i, ch) in input.char_indices() {
        if ch == '{' {
            let mut depth = 0i32;
            let mut in_string = false;
            let mut escape = false;

            for (offset, c) in input[i..].char_indices() {
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
                            return &input[i..i + offset + c.len_utf8()];
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    input.trim()
}

/// 转义 JSON 字符串值中的游离 ASCII 双引号
///
/// LLM（尤其是 MiniMax）在中文对话/叙事中经常输出形如：
/// `"声如洪钟："这位兄台""` — 这里的内部 `"` 未被转义，
/// 导致 serde_json 提前截断字符串。
///
/// 算法：逐字符扫描 JSON，维护字符串状态。遇到 `"` 时判断它是
/// **结构引号**（字符串边界）还是**内容引号**（需要转义）：
///
/// 判定规则 — 如果 `"` 后面的第一个非空白字符是以下之一，则为结构引号：
/// - `:` → key 结束
/// - `,` → value 结束
/// - `}` → object 结束
/// - `]` → array 结束
/// - `\0` (字符串结束) → 最后一个 value
///
/// 否则为内容引号，转义为 `\"`。
fn escape_unescaped_quotes(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut output = String::with_capacity(input.len() + 64);
    let mut in_string = false;
    let mut escape = false;
    let mut i = 0;

    while i < len {
        let c = chars[i];

        if escape {
            output.push('\\');
            output.push(c);
            escape = false;
            i += 1;
            continue;
        }

        if c == '\\' && in_string {
            // 标记下一个字符为已转义，不提前输出
            escape = true;
            i += 1;
            continue;
        }

        if c == '"' {
            if !in_string {
                // 进入字符串
                in_string = true;
                output.push('"');
                i += 1;
            } else {
                // 可能是字符串结束，也可能是内容引号
                // 检查后面的第一个非空白字符
                let after = skip_whitespace(&chars, i + 1);
                if after < len && is_structural_char(chars[after]) {
                    // 结构引号 — 字符串正常结束
                    in_string = false;
                    output.push('"');
                    i += 1;
                } else {
                    // 内容引号 — 转义它
                    output.push('\\');
                    output.push('"');
                    i += 1;
                }
            }
            continue;
        }

        // 普通字符
        output.push(c);
        i += 1;
    }

    output
}

/// 跳过空白字符，返回下一个非空白字符的索引
fn skip_whitespace(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// 判断字符是否为 JSON 结构字符（出现在字符串结束之后）
fn is_structural_char(c: char) -> bool {
    matches!(c, ':' | ',' | '}' | ']' | '{' | '[')
}

/// 清洗 LLM 响应中的非法控制字符
///
/// JSON 规范仅允许 \t (\x09), \n (\x0A), \r (\x0D)。
/// 其他控制字符替换为空格。
fn sanitize_control_chars(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\r' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .collect()
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

        // Step 1: 提取 JSON 范围（宽松匹配，不要求内部引号合法）
        let json_str = extract_json_span(&response);

        // Step 2: 清洗控制字符
        let cleaned = sanitize_control_chars(json_str);

        // Step 3: 尝试直接解析
        if let Ok(parsed) = serde_json::from_str::<D>(&cleaned) {
            return Ok(parsed);
        }

        // Step 4: 转义游离引号后重试
        tracing::warn!("[complete_json] Direct parse failed, escaping unescaped quotes...");
        let escaped = escape_unescaped_quotes(&cleaned);
        match serde_json::from_str::<D>(&escaped) {
            Ok(parsed) => {
                tracing::info!("[complete_json] Quote escape repair succeeded");
                return Ok(parsed);
            }
            Err(e) => {
                tracing::error!(
                    "[complete_json] All repair attempts failed: {}\nOriginal: {}\nEscaped: {}",
                    e,
                    cleaned,
                    escaped
                );
                Err(anyhow::anyhow!(
                    "[complete_json] Failed to parse LLM JSON after all repair attempts"
                ))
            }
        }
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

    // --- 新增：覆盖实测 bug 的回归测试 ---

    /// 测试：LLM 在字符串值中插入未转义 ASCII 引号
    ///
    /// 还原 bug：MiniMax 输出 `"声如洪钟："这位兄台""`
    /// 其中 `："` 的 `"` 未转义，导致 serde_json 提前截断
    #[tokio::test]
    async fn test_complete_json_unescaped_quotes() {
        #[derive(serde::Deserialize)]
        struct ReviewResponse {
            result: String,
            #[allow(dead_code)]
            reason: String,
            #[allow(dead_code)]
            narrative: String,
        }

        // 模拟 MiniMax 实际输出：narrative 中有未转义 "
        let raw = r#"{
  "result": "approved",
  "reason": "符合人设",
  "narrative": "午后暖阳斜照，赵无极步入大堂。他抱拳一礼，声如洪钟："这位兄台，在下赵无极。敢问这镇上可有铸造师？"说罢，他拍了拍腰间重剑。"
}"#;

        let client = MockLlmClient::with_response(raw);
        let result: ReviewResponse = client.complete_json("test").await.unwrap();
        assert_eq!(result.result, "approved");
        assert!(result.narrative.contains("赵无极"));
    }

    /// 测试：LLM 输出包含控制字符
    #[tokio::test]
    async fn test_complete_json_control_chars() {
        #[derive(serde::Deserialize)]
        struct SimpleResponse {
            message: String,
        }

        // \x01 和 \x02 是非法控制字符
        let raw = "{\"message\": \"hello\x01world\x02test\"}";
        let client = MockLlmClient::with_response(raw);
        let result: SimpleResponse = client.complete_json("test").await.unwrap();
        assert_eq!(result.message, "hello world test");
    }

    /// 测试：正常 JSON 不被修改
    #[tokio::test]
    async fn test_complete_json_normal_passthrough() {
        #[derive(serde::Deserialize)]
        struct NormalResponse {
            action: String,
            #[allow(dead_code)]
            narrative: String,
        }

        let raw = r#"{"action": "speak", "narrative": "赵无极拱手行礼说道：在下初来乍到。"}"#;
        let client = MockLlmClient::with_response(raw);
        let result: NormalResponse = client.complete_json("test").await.unwrap();
        assert_eq!(result.action, "speak");
    }

    /// 测试：LLM 在 JSON 前输出分析文字
    #[tokio::test]
    async fn test_complete_json_with_prose() {
        #[derive(serde::Deserialize)]
        struct Decision {
            action: String,
        }

        let raw = "让我分析一下当前情况...\nAgent 应该优先社交。\n\n{\"action\": \"speak\"}";
        let client = MockLlmClient::with_response(raw);
        let result: Decision = client.complete_json("test").await.unwrap();
        assert_eq!(result.action, "speak");
    }

    /// 测试：escape_unescaped_quotes 单元测试
    #[test]
    fn test_escape_unescaped_quotes_basic() {
        // narrative 值中有两个未转义 "
        let input = r#"{"result": "approved", "narrative": "声如洪钟："这位兄台。"}"#;
        let output = escape_unescaped_quotes(input);
        // 验证输出是合法 JSON
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["result"], "approved");
        assert!(parsed["narrative"].as_str().unwrap().contains("这位兄台"));
    }

    #[test]
    fn test_escape_unescaped_quotes_preserves_escaped() {
        // 已转义的 \" 不应被双重转义
        // 用 serde_json 构造合法 JSON，确保 \" 存在于输入中
        let original = serde_json::json!({"msg": "hello \"world\" end"});
        let input = original.to_string();
        // input 此时是 {"msg":"hello \"world\" end"}，其中 \" 是合法转义
        let output = escape_unescaped_quotes(&input);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["msg"], "hello \"world\" end");
    }

    #[test]
    fn test_escape_unescaped_quotes_multiple_fields() {
        let input = r#"{"a": "x："y", "b": "normal"}"#;
        let output = escape_unescaped_quotes(input);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["b"], "normal");
    }
}
