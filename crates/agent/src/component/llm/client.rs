// ============================================================================
// LLM 客户端接口与 Fallback 降级
// ============================================================================
//
// 定义 LLM 客户端 Trait (LlmClient) 及其两种实现：
// - DirectLlmClient: 直接调用 LLM API（single model）
// - FallbackLlmClient: 多模型包装器，主模型 403/超时时自动降级
//
// FallbackLlmClient 策略：
// - 按序尝试所有模型（主模型 → fallback_models）
// - 成功后 sticky 到该模型（避免反复切换）
// - 仅对可恢复错误（403/429/超时）触发 fallback
// ============================================================================

use anyhow::Result;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use std::sync::Arc;

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

    /// 是否支持 tool calling
    fn supports_tool_calling(&self) -> bool {
        false
    }

    /// 强制切换到下一个模型（用于连续 idle 时主动换模型）
    ///
    /// 返回 `true` 表示成功切换，`false` 表示只有单模型无法切换。
    /// 默认实现返回 `false`（单模型客户端无需切换）。
    fn force_rotate_model(&self) -> bool {
        false
    }

    /// 使用 tool calling 的多轮对话
    ///
    /// 如果 LLM 返回 tool_calls，调用 executor 执行后继续对话，
    /// 直到 LLM 返回最终文本响应或超过 max_rounds。
    async fn complete_with_tools(
        &self,
        system: &str,
        prompt: &str,
        tools: &[super::tool_types::ToolDefinition],
        executor: &dyn super::tool_types::ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let _ = (system, prompt, tools, executor, max_rounds);
        anyhow::bail!("Tool calling not supported by this LLM client")
    }
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

    /// 使用 tool calling 的多轮对话，返回结构化 JSON
    async fn complete_json_with_tools<T: DeserializeOwned + Send>(
        &self,
        system: &str,
        prompt: &str,
        tools: &[super::tool_types::ToolDefinition],
        executor: &dyn super::tool_types::ToolExecutor,
        max_rounds: usize,
    ) -> Result<T>;
}

/// 剥离 LLM 响应中的 thinking/reasoning 标签
///
/// 部分 LLM（如 MiniMax）在非流式调用时会在 JSON 前输出思考过程，
/// 导致 `find('{')` 匹配到 thinking 内容中的 `{` 而非 JSON 的 `{`。
fn strip_thinking_tags(response: &str) -> std::borrow::Cow<'_, str> {
    // 匹配配对标签: <think_tag>...</think_tag>, <think attrs>...</think*>, <reasoning>...</reasoning>, <thought>...</thought>
    let paired_re = regex::Regex::new(
        r"(?is)<(?:think_tag|think|reasoning|thought)[^>]*>.*?</(?:think_tag|think|reasoning|thought)\s*>"
    ).unwrap();

    let cleaned = paired_re.replace_all(response, "").to_string();

    // 处理自闭合标签: <think/>, <think />, <think.../>, <think length="123"/>
    let self_closing_re = regex::Regex::new(r"(?i)<(?:think_tag|think|reasoning|thought)[^>]*/>\s*").unwrap();
    let cleaned = self_closing_re.replace_all(&cleaned, "").to_string();

    if cleaned == response {
        std::borrow::Cow::Borrowed(response)
    } else {
        std::borrow::Cow::Owned(cleaned)
    }
}

/// 从 LLM 响应中提取 JSON 字符串
///
/// 使用大括号计数找第一个完整 JSON 对象，避免 LLM 在 JSON 后输出额外内容
/// 导致 "trailing characters" 解析错误（如 MiniMax 输出多行 JSON）。
fn extract_json_str(response: &str) -> std::borrow::Cow<'_, str> {
    // 先剥离 thinking tags
    let cleaned = strip_thinking_tags(response);

    let response = cleaned.as_ref();

    if let Some(start) = response.find("```json") {
        let after_marker = start + 7;
        if let Some(end) = response[after_marker..].find("```") {
            std::borrow::Cow::Owned(
                response[after_marker..after_marker + end]
                    .trim()
                    .to_string(),
            )
        } else {
            std::borrow::Cow::Owned(response[after_marker..].trim().to_string())
        }
    } else if let Some(end) = find_first_json_end(response) {
        std::borrow::Cow::Owned(response[..=end].to_string())
    } else {
        std::borrow::Cow::Owned(response.trim().to_string())
    }
}

/// 用大括号计数找第一个完整 JSON 对象的结束位置
///
/// 从第一个 `{` 开始，逐字符追踪大括号深度（跳过字符串内的大括号），
/// 当深度归零时即为第一个完整 JSON 对象的末尾。
fn find_first_json_end(s: &str) -> Option<usize> {
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for (i, &c) in s.as_bytes().iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        if c == b'\\' && in_string {
            escape = true;
            continue;
        }
        if c == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
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

    match serde_json::from_str::<D>(&json_str) {
        Ok(parsed) => Ok(parsed),
        Err(first_err) => {
            let fixed = try_fix_truncated_json(&json_str);

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

    async fn complete_json_with_tools<D: DeserializeOwned + Send>(
        &self,
        system: &str,
        prompt: &str,
        tools: &[super::tool_types::ToolDefinition],
        executor: &dyn super::tool_types::ToolExecutor,
        max_rounds: usize,
    ) -> Result<D> {
        let text = self
            .complete_with_tools(system, prompt, tools, executor, max_rounds)
            .await?;
        parse_json_response::<D>(&text)
    }
}

// ============================================================================
// Fallback LLM 客户端（403/超时自动降级）
// ============================================================================

/// Fallback LLM 客户端
///
/// 主模型 403（额度耗尽）或超时时，自动切换到备用模型。
/// 所有模型共享同一 provider/api_key，仅 model name 不同。
///
/// 一旦某个 fallback 成功，后续调用优先使用该模型（sticky fallback）。
pub struct FallbackLlmClient {
    /// LLM 客户端列表（index 0 = 主模型，1.. = fallback）
    clients: Vec<Arc<dyn LlmClient>>,
    /// 当前活跃客户端索引
    active: Arc<std::sync::atomic::AtomicUsize>,
}

impl FallbackLlmClient {
    /// 创建 Fallback 客户端
    ///
    /// `clients` 不应为空，index 0 是主模型。
    pub fn new(clients: Vec<Arc<dyn LlmClient>>) -> Self {
        assert!(
            !clients.is_empty(),
            "FallbackLlmClient needs at least one client"
        );
        Self {
            clients,
            active: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// 强制切换到下一个模型
    ///
    /// 将 active 索引前进一位（环绕）。返回 true 表示切换成功，
    /// false 表示只有一个模型无法切换。
    pub fn force_rotate(&self) -> bool {
        if self.clients.len() <= 1 {
            return false;
        }
        let old = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let new = (old + 1) % self.clients.len();
        self.active.store(new, std::sync::atomic::Ordering::Relaxed);
        tracing::warn!("强制切换 LLM 模型: #{} → #{}", old, new);
        true
    }

    /// 获取当前活跃客户端
    fn active_client(&self) -> Arc<dyn LlmClient> {
        let idx = self.active.load(std::sync::atomic::Ordering::Relaxed);
        self.clients[idx.min(self.clients.len() - 1)].clone()
    }

    /// 判断错误是否应触发 fallback
    ///
    /// 匹配条件：
    /// - HTTP 403 (AllocationQuota / 额度耗尽)
    /// - HTTP 429 (Rate limit)
    /// - 空响应（模型返回 null/空内容）
    fn should_fallback(error: &anyhow::Error) -> bool {
        let msg = format!("{:#}", error);
        // HTTP 状态码匹配（直接来自 API 响应）
        msg.contains("LLM API error 403")
            || msg.contains("LLM API error 429")
            // 额度耗尽关键词
            || msg.contains("AllocationQuota")
            // 连接/请求失败（.context() 包装后的前缀）
            || msg.contains("Failed to send request to LLM API")
            // 空响应（MiniMax 等模型偶尔返回 content=null）
            || msg.contains("response content is empty")
    }

    /// 执行带 fallback 的调用
    ///
    /// 策略：从 active index 开始，失败时尝试后续所有客户端。
    /// 一旦成功，sticky 到该客户端。
    async fn call_with_fallback<F, Fut>(&self, f: F) -> Result<String>
    where
        F: Fn(Arc<dyn LlmClient>) -> Fut,
        Fut: std::future::Future<Output = Result<String>>,
    {
        let start = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let mut last_err = None;

        for offset in 0..self.clients.len() {
            let idx = (start + offset) % self.clients.len();
            let client = self.clients[idx].clone();

            match f(client).await {
                Ok(response) => {
                    if offset > 0 {
                        tracing::warn!(
                            "LLM fallback 成功：切换到客户端 #{} (主用 #{}）",
                            idx,
                            start
                        );
                        // sticky：后续调用使用此客户端
                        self.active.store(idx, std::sync::atomic::Ordering::Relaxed);
                    }
                    return Ok(response);
                }
                Err(e) => {
                    let should = Self::should_fallback(&e);
                    tracing::warn!("LLM 客户端 #{} 调用失败 (fallback={}: {}", idx, should, e);
                    if !should {
                        // 非 fallback 类错误（如 JSON 解析失败），直接返回
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        }

        // 所有客户端都失败
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("所有 LLM 客户端均失败")))
    }
}

#[async_trait]
impl LlmClient for FallbackLlmClient {
    fn force_rotate_model(&self) -> bool {
        self.force_rotate()
    }

    async fn complete(&self, prompt: &str) -> Result<String> {
        let prompt = prompt.to_string();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let prompt = prompt.clone();
            async move { client.complete(&prompt).await }
        })
        .await
    }

    async fn complete_with_system(&self, system: &str, prompt: &str) -> Result<String> {
        let system = system.to_string();
        let prompt = prompt.to_string();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let system = system.clone();
            let prompt = prompt.clone();
            async move { client.complete_with_system(&system, &prompt).await }
        })
        .await
    }

    fn supports_tool_calling(&self) -> bool {
        self.active_client().supports_tool_calling()
    }

    async fn complete_with_tools(
        &self,
        system: &str,
        prompt: &str,
        tools: &[super::tool_types::ToolDefinition],
        executor: &dyn super::tool_types::ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let system = system.to_string();
        let prompt = prompt.to_string();
        let tools = tools.to_vec();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let system = system.clone();
            let prompt = prompt.clone();
            let tools = tools.clone();
            async move {
                client
                    .complete_with_tools(&system, &prompt, &tools, executor, max_rounds)
                    .await
            }
        })
        .await
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

    // ========================================================================
    // find_first_json_end tests
    // ========================================================================

    #[test]
    fn test_find_json_simple() {
        let s = r#"{"a":1}"#;
        assert_eq!(find_first_json_end(s), Some(6));
    }

    #[test]
    fn test_find_json_with_trailing() {
        let s = r#"{"a":1}{"b":2}"#;
        assert_eq!(find_first_json_end(s), Some(6));
    }

    #[test]
    fn test_find_json_nested() {
        let s = r#"{"a":{"b":2}}"#;
        assert_eq!(find_first_json_end(s), Some(12));
    }

    #[test]
    fn test_find_json_string_with_braces() {
        let s = r#"{"a":"{b}"}"#;
        assert_eq!(find_first_json_end(s), Some(10));
    }

    #[test]
    fn test_find_json_escaped_quotes() {
        let s = r#"{"a":"he said \"hello\""}"#;
        assert_eq!(find_first_json_end(s), Some(24));
    }

    #[test]
    fn test_find_json_no_object() {
        let s = "no json here";
        assert_eq!(find_first_json_end(s), None);
    }

    #[test]
    fn test_find_json_with_prefix() {
        let s = r#"some text {"a":1}"#;
        assert_eq!(find_first_json_end(s), Some(16));
    }

    // ========================================================================
    // strip_thinking_tags tests
    // ========================================================================

    #[test]
    fn test_strip_think_tag() {
        let input = "<think_tag>reasoning</think_tag>{\"a\":1}";
        let result = strip_thinking_tags(input);
        assert_eq!(result.as_ref(), "{\"a\":1}");
    }

    #[test]
    fn test_strip_reasoning_tag() {
        let input = "<reasoning>let me think...</reasoning>{\"result\":42}";
        let result = strip_thinking_tags(input);
        assert_eq!(result.as_ref(), "{\"result\":42}");
    }

    #[test]
    fn test_strip_no_tags() {
        let input = "{\"a\":1}";
        let result = strip_thinking_tags(input);
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
        assert_eq!(result.as_ref(), "{\"a\":1}");
    }

    #[test]
    fn test_strip_empty_response() {
        let input = "";
        let result = strip_thinking_tags(input);
        assert!(result.is_empty());
    }

    // MiniMax-M2.7 实际输出格式
    #[test]
    fn test_strip_minimax_think_with_attrs() {
        let input = r#"<think.../>
思考过程...

{"action_type": "eat", "action_data": {"item_id": "mantou"}, "speech_content": ""}"#;
        let result = strip_thinking_tags(input);
        assert!(result.contains(r#"{"action_type": "eat"#), "应保留 JSON，实际: {}", result);
        assert!(!result.contains("<think"), "应移除 think 标签，实际: {}", result);
    }

    #[test]
    fn test_strip_think_self_closing() {
        let input = "<think/>{\"a\":1}";
        let result = strip_thinking_tags(input);
        assert_eq!(result.as_ref(), "{\"a\":1}");
    }

    #[test]
    fn test_strip_think_with_spaces() {
        let input = "<think />{\"a\":1}";
        let result = strip_thinking_tags(input);
        assert_eq!(result.as_ref(), "{\"a\":1}");
    }

    #[test]
    fn test_strip_thought_tag() {
        let input = "<thought>思考中...</thought>{\"a\":1}";
        let result = strip_thinking_tags(input);
        assert_eq!(result.as_ref(), "{\"a\":1}");
    }

    #[test]
    fn test_strip_minimax_full_response() {
        // 完整 MiniMax 输出：self-closing think 后跟思考文字和 JSON
        // <think.../> 是自闭合标签，后面的思考文字是普通文本（非标签包裹）
        // extract_json_str 会通过 find_first_json_end 定位到 JSON
        let input = "<think.../>\n考虑拾取馒头充饥\n\n{\"action_type\": \"drink\", \"action_data\": {\"item_id\": \"water\"}, \"speech_content\": \"\"}";
        let result = strip_thinking_tags(input);
        // 自闭合标签被移除，但思考文本仍在（非标签包裹无法剥离）
        assert!(!result.contains("<think"), "think 标签应被移除");
        assert!(result.contains("\"action_type\": \"drink\""), "JSON 应保留");
    }
}
