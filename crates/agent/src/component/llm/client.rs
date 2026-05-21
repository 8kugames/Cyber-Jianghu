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

/// 对话轮次（用于长窗口对话）
#[derive(Debug, Clone)]
pub struct ConversationTurn {
    pub user: String,
    pub assistant: String,
}

/// 对话输入参数（用于减少函数参数数量）
#[derive(Debug, Clone)]
pub struct ConversationInput<'a> {
    /// 对话历史摘要
    pub summary: Option<&'a str>,
    /// 保留的近期完整轮次
    pub turns: &'a [ConversationTurn],
    /// 当前请求的 prompt
    pub current_prompt: &'a str,
}

/// 构建对话消息列表（system + summary + history + current prompt）
pub fn build_conversation_messages(
    system: &str,
    summary: Option<&str>,
    turns: &[ConversationTurn],
    current_prompt: &str,
) -> Vec<super::openai_types::ChatMessage> {
    use super::openai_types::ChatMessage;

    let mut system_content = system.to_string();
    if let Some(s) = summary {
        system_content.push_str(&format!("\n\n## 对话历史摘要\n{}", s));
    }

    let mut messages = vec![ChatMessage::system(&system_content)];
    for turn in turns {
        messages.push(ChatMessage::user(&turn.user));
        messages.push(ChatMessage::assistant(&turn.assistant));
    }
    messages.push(ChatMessage::user(current_prompt));
    messages
}

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

    /// 原始消息交换：发送消息列表 + 可选工具定义，返回 LLM 响应
    ///
    /// 这是 LLM 接入点的唯一抽象 — DirectLlmClient 用 HTTP，
    /// OpenClawBridge 用 WebSocket。循环逻辑不应在此。
    #[allow(private_interfaces)]
    async fn send_chat_exchange(
        &self,
        messages: Vec<super::openai_types::ChatMessage>,
        tools: Option<&[super::tool_types::ToolDefinition]>,
        config: super::openai_types::ChatExchangeConfig,
    ) -> Result<super::openai_types::ChatExchangeResponse> {
        let _ = (messages, tools, config);
        anyhow::bail!("Chat exchange not supported by this LLM client")
    }

    /// 强制切换到下一个模型（用于连续 idle 时主动换模型）
    ///
    /// 返回 `true` 表示成功切换，`false` 表示只有单模型无法切换。
    /// 默认实现返回 `false`（单模型客户端无需切换）。
    fn force_rotate_model(&self) -> bool {
        false
    }

    /// 记录当前模型返回 idle，自动切换到下一个模型
    ///
    /// 如果当前模型连续 idle 达到阈值，则标记为不可用并切换。
    /// 返回 true 表示发生了切换，false 表示未达到阈值。
    /// 默认实现不做任何操作（单模型客户端无需切换）。
    fn record_idle(&self) -> bool {
        false
    }

    /// 重置当前模型的 idle 计数（当模型返回非 idle 结果时调用）
    ///
    /// 默认实现不做任何操作。
    fn reset_idle_count(&self) {
        // 默认不做任何操作
    }

    /// 获取 provider 名称（用于 token 统计）
    ///
    /// 默认实现返回 "unknown"。
    fn provider_name(&self) -> String {
        "unknown".to_string()
    }

    /// 获取模型名称（用于 token 统计）
    ///
    /// 默认实现返回 "unknown"。
    fn model_name(&self) -> String {
        "unknown".to_string()
    }

    /// 获取 (provider, model) 元组（用于 token 统计兜底记录）
    fn provider_info(&self) -> (super::direct_client::LlmProvider, String) {
        (
            super::direct_client::LlmProvider::OpenClaw,
            "unknown".to_string(),
        )
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

    /// 使用对话历史 + tool calling 的组合调用
    ///
    /// 结合 `complete_with_conversation` 和 `complete_with_tools`：
    /// 消息列表包含对话历史，同时 LLM 可调用工具。
    /// 默认退化：忽略对话历史，委托给 `complete_with_tools`。
    async fn complete_with_conversation_and_tools(
        &self,
        system: &str,
        input: ConversationInput<'_>,
        tools: &[super::tool_types::ToolDefinition],
        executor: &dyn super::tool_types::ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let _ = (&input.summary, input.turns);
        self.complete_with_tools(system, input.current_prompt, tools, executor, max_rounds)
            .await
    }

    /// 使用对话历史完成调用（长窗口）
    ///
    /// `summary` 为旧轮次的压缩摘要（注入 system message）。
    /// `turns` 为保留的近期完整轮次。
    /// `current_prompt` 为当前 tick 的用户输入。
    ///
    /// 默认实现退化为 system + current_prompt（不使用历史）。
    async fn complete_with_conversation(
        &self,
        system: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<String> {
        let _ = (summary, turns);
        self.complete_with_system(system, current_prompt).await
    }

    /// 流式完成（system + user），返回 SSE 流
    ///
    /// 默认实现退化为非流式（包装为单 chunk 流）。
    fn complete_streaming<'a>(
        &'a self,
        system: &'a str,
        prompt: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<super::streaming::LlmStream>> + Send + 'a>,
    > {
        Box::pin(async move {
            let result = self.complete_with_system(system, prompt).await?;
            let stream = futures_util::stream::once(async move {
                Ok(super::streaming::StreamChunk::Delta(result))
            });
            let boxed: super::streaming::LlmStream = Box::pin(stream);
            Ok(boxed)
        })
    }

    /// 流式对话完成（长窗口），返回 SSE 流
    ///
    /// 默认实现退化为非流式（包装为单 chunk 流）。
    fn complete_conversation_streaming<'a>(
        &'a self,
        system: &'a str,
        summary: Option<&'a str>,
        turns: &'a [ConversationTurn],
        current_prompt: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<super::streaming::LlmStream>> + Send + 'a>,
    > {
        Box::pin(async move {
            let result = self
                .complete_with_conversation(system, summary, turns, current_prompt)
                .await?;
            let stream = futures_util::stream::once(async move {
                Ok(super::streaming::StreamChunk::Delta(result))
            });
            let boxed: super::streaming::LlmStream = Box::pin(stream);
            Ok(boxed)
        })
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

    /// 使用对话历史完成结构化输出（长窗口）
    async fn complete_json_with_conversation<T: DeserializeOwned + Send>(
        &self,
        system: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<T>;

    /// 流式完成结构化输出（system + user）
    ///
    /// 内部消费 SSE 流，累积文本，JSON 闭合后早期终止。
    async fn complete_json_streaming<T: DeserializeOwned + Send>(
        &self,
        system: &str,
        prompt: &str,
    ) -> Result<T>;

    /// 流式对话完成结构化输出（长窗口）
    ///
    /// 内部消费 SSE 流，累积文本，JSON 闭合后早期终止。
    async fn complete_json_streaming_with_conversation<T: DeserializeOwned + Send>(
        &self,
        system: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<T>;

    /// 使用对话历史 + tool calling 的结构化输出
    async fn complete_json_with_conversation_and_tools<D: DeserializeOwned + Send>(
        &self,
        system: &str,
        input: ConversationInput<'_>,
        tools: &[super::tool_types::ToolDefinition],
        executor: &dyn super::tool_types::ToolExecutor,
        max_rounds: usize,
    ) -> Result<D>;
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
    let self_closing_re =
        regex::Regex::new(r"(?i)<(?:think_tag|think|reasoning|thought)[^>]*/>\s*").unwrap();
    let cleaned = self_closing_re.replace_all(&cleaned, "").to_string();

    if cleaned == response {
        std::borrow::Cow::Borrowed(response)
    } else {
        std::borrow::Cow::Owned(cleaned)
    }
}

/// Normalize LLM 输出中错误转义的双花括号
///
/// 部分 LLM（如 LongCat-2.0-Preview）将 JSON 结构中的 `{` 输出为 `{{`。
/// 在本项目领域（游戏动作 JSON）中，字符串值内不会出现 `{{`，因此全局替换安全。
pub(super) fn normalize_double_braces(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains("{{") || s.contains("}}") {
        std::borrow::Cow::Owned(s.replace("{{", "{").replace("}}", "}"))
    } else {
        std::borrow::Cow::Borrowed(s)
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

    let normalized = normalize_double_braces(response);
    let response = normalized.as_ref();

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

/// LLM JSON 归一化预处理
///
/// 确定性文本变换，将 LLM 输出的常见非标 JSON 模式归一化为合法 JSON。
/// 在 serde_json::from_str 之前调用，消除因模型输出质量导致的解析失败。
///
/// 处理范围：
/// 1. 中文全角引号 "" → ""
/// 2. 单行注释 // ... → 移除
/// 3. 尾部逗号 → 移除
/// 4. 未闭合引号/括号 → 修补
fn normalize_llm_json(json_str: &str) -> String {
    let mut result = String::with_capacity(json_str.len());
    let mut chars = json_str.chars().peekable();
    let mut in_string = false;

    while let Some(c) = chars.next() {
        if in_string {
            if c == '\\' {
                result.push('\\');
                if let Some(next) = chars.next() {
                    result.push(next);
                }
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            result.push(c);
            continue;
        }

        match c {
            '"' => {
                in_string = true;
                result.push('"');
            }
            // 中文左/右引号 → "
            '\u{201c}' | '\u{201d}' => {
                result.push('"');
            }
            // 单行注释 // ... \n → 移除到行尾
            '/' if chars.peek() == Some(&'/') => {
                for nc in chars.by_ref() {
                    if nc == '\n' {
                        break;
                    }
                }
            }
            _ => {
                result.push(c);
            }
        }
    }

    // 尾部清理
    let mut fixed = result.trim_end().to_string();

    // 移除数组/对象闭合后的多余引号：]" → ], }" → }
    let bytes = fixed.as_bytes();
    let len = bytes.len();
    if len >= 2 {
        let last = bytes[len - 1];
        let prev = bytes[len - 2];
        if last == b'"' && (prev == b']' || prev == b'}') {
            fixed.pop();
        }
    }

    // 移除尾部逗号（在 } 或 ] 前的）
    while let Some(last) = fixed.chars().last() {
        if last == ',' {
            fixed.pop();
            fixed = fixed.trim_end().to_string();
        } else {
            break;
        }
    }

    // 修复未闭合引号（奇数个 "）
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

/// 解析 LLM 响应为结构化类型（带归一化预处理 + 括号平衡修复）
///
/// 流程：extract_json_str → normalize_llm_json → 括号平衡修复 → serde 解析
///
/// 括号平衡修复：逐字符追踪 {}[] 嵌套深度（跳过字符串内部），
/// 当遇到 ] 或 } 试图闭合但深度不匹配时，自动补全缺失的闭合符号。
/// 这是确定性的 — 不猜测，直接修正嵌套错误。
fn parse_json_response<D: DeserializeOwned + Send>(response: &str) -> Result<D> {
    let raw_json = extract_json_str(response);
    let json_str = normalize_llm_json(&raw_json);

    // 第一轮：严格解析
    if let Ok(parsed) = serde_json::from_str::<D>(&json_str) {
        return Ok(parsed);
    }

    // 第二轮：括号平衡修复
    let balanced = balance_braces(&json_str);
    if balanced != json_str
        && let Ok(parsed) = serde_json::from_str::<D>(&balanced)
    {
        tracing::warn!("JSON repaired via brace balancing");
        return Ok(parsed);
    }

    // 全部失败，输出诊断信息
    let strict_err = match serde_json::from_str::<D>(&json_str) {
        Ok(_) => unreachable!(),
        Err(e) => e,
    };
    let error_line = strict_err.line();
    let lines: Vec<&str> = json_str.lines().collect();
    let start = error_line.saturating_sub(4);
    let end = (error_line + 2).min(lines.len());
    let error_snippet: String = lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let line_num = start + i + 1;
            let marker = if line_num == error_line { ">>>" } else { "   " };
            format!("{} {:4}: {}", marker, line_num, l)
        })
        .collect::<Vec<_>>()
        .join("\n");

    tracing::error!(
        error_type = ?strict_err.classify(),
        error_msg = %strict_err,
        line = error_line,
        column = strict_err.column(),
        json_len = json_str.len(),
        "\n{error_snippet}\n--- Full JSON ---\n{json_str}"
    );
    Err(strict_err.into())
}

/// 括号平衡修复：确定性深度追踪 + 自动补全
///
/// 逐字符追踪 {}[] 嵌套深度（跳过字符串和转义序列），
/// 核心逻辑：遇到 `}` 时只弹出一个 `{`（不跨类型），遇到 `]` 时只弹出一个 `[`。
/// 栈空时遇到闭合符号 → 丢弃（LLM 多余输出）。
/// 额外修复：当 `}` 后跟随 `,` + `{`，如果栈深度表明当前对象未闭合，立即补 `}`。
fn balance_braces(json: &str) -> String {
    let chars: Vec<char> = json.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(json.len() + 32);
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut i = 0;

    while i < len {
        let c = chars[i];

        if in_string {
            result.push(c);
            if c == '\\' && i + 1 < len {
                i += 1;
                result.push(chars[i]);
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        match c {
            '"' => {
                in_string = true;
                result.push('"');
            }
            '{' | '[' => {
                stack.push(c);
                result.push(c);
            }
            '}' => {
                if stack.is_empty() {
                    i += 1;
                    continue;
                }
                while let Some(&top) = stack.last() {
                    if top == '{' {
                        stack.pop();
                        break;
                    }
                    stack.pop();
                    result.push(']');
                }
                result.push('}');

                // }, { 模式：action_data 的 } 关闭后，紧跟 ,{ 表示下一个元素
                let mut peek = i + 1;
                while peek < len && chars[peek].is_whitespace() {
                    peek += 1;
                }
                if peek < len && chars[peek] == ',' {
                    peek += 1;
                    while peek < len && chars[peek].is_whitespace() {
                        peek += 1;
                    }
                    if peek < len && chars[peek] == '{' {
                        while stack.last() == Some(&'{') {
                            stack.pop();
                            result.push('}');
                        }
                    }
                }
            }
            ']' => {
                if stack.is_empty() {
                    i += 1;
                    continue;
                }
                while let Some(&top) = stack.last() {
                    if top == '[' {
                        stack.pop();
                        break;
                    }
                    stack.pop();
                    result.push('}');
                }
                result.push(']');
            }
            _ => {
                result.push(c);
            }
        }
        i += 1;
    }

    while let Some(top) = stack.pop() {
        result.push(if top == '{' { '}' } else { ']' });
    }

    result
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

    async fn complete_json_with_conversation_and_tools<D: DeserializeOwned + Send>(
        &self,
        system: &str,
        input: ConversationInput<'_>,
        tools: &[super::tool_types::ToolDefinition],
        executor: &dyn super::tool_types::ToolExecutor,
        max_rounds: usize,
    ) -> Result<D> {
        let text = self
            .complete_with_conversation_and_tools(system, input, tools, executor, max_rounds)
            .await?;
        parse_json_response::<D>(&text)
    }

    async fn complete_json_with_conversation<D: DeserializeOwned + Send>(
        &self,
        system: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<D> {
        let response = self
            .complete_with_conversation(system, summary, turns, current_prompt)
            .await?;
        parse_json_response::<D>(&response)
    }

    async fn complete_json_streaming<D: DeserializeOwned + Send>(
        &self,
        system: &str,
        prompt: &str,
    ) -> Result<D> {
        use futures_util::StreamExt;

        let stream = self.complete_streaming(system, prompt).await?;
        let mut acc = super::streaming::StreamAccumulator::new();
        let mut stream = std::pin::pin!(stream);
        let mut json_complete = false;

        // 必须完全耗尽流以确保 Done chunk (含 usage) 被处理
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            // JSON 完成后仍继续累积，确保收到 Done chunk
            if !json_complete {
                json_complete = acc.is_json_complete();
            }
            acc.push(chunk);
        }

        let (pt, ct, has_real) = acc.token_stats();
        if pt > 0 || ct > 0 {
            tracing::debug!(
                "Streaming JSON token usage: prompt={}, completion={}, real={}",
                pt,
                ct,
                has_real
            );
        }

        let content = acc.content();
        if content.trim().is_empty() {
            anyhow::bail!(
                "LLM API error: response content is empty (streaming_json, prompt_tokens={}, completion_tokens={})",
                pt,
                ct
            );
        }

        parse_json_response::<D>(content)
    }

    async fn complete_json_streaming_with_conversation<D: DeserializeOwned + Send>(
        &self,
        system: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<D> {
        use futures_util::StreamExt;

        let stream = self
            .complete_conversation_streaming(system, summary, turns, current_prompt)
            .await?;
        let mut acc = super::streaming::StreamAccumulator::new();
        let mut stream = std::pin::pin!(stream);
        let mut json_complete = false;

        // 必须完全耗尽流以确保 Done chunk (含 usage) 被处理
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            // JSON 完成后仍继续累积，确保收到 Done chunk
            if !json_complete {
                json_complete = acc.is_json_complete();
            }
            acc.push(chunk);
        }

        let (pt, ct, has_real) = acc.token_stats();
        if pt > 0 || ct > 0 {
            tracing::debug!(
                "Streaming JSON conv token usage: prompt={}, completion={}, real={}",
                pt,
                ct,
                has_real
            );
        }

        let content = acc.content();
        if content.trim().is_empty() {
            anyhow::bail!(
                "LLM API error: response content is empty (streaming_json_conv, prompt_tokens={}, completion_tokens={})",
                pt,
                ct
            );
        }

        parse_json_response::<D>(content)
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
///
/// Idle 旋转机制：连续 idle 达到阈值时自动切换到下一个模型。
pub struct FallbackLlmClient {
    /// LLM 客户端列表（index 0 = 主模型，1.. = fallback）
    clients: Vec<Arc<dyn LlmClient>>,
    /// 当前活跃客户端索引
    active: Arc<std::sync::atomic::AtomicUsize>,
    /// 连续 idle 计数（每个模型独立计数）
    idle_counts: Arc<std::sync::Mutex<Vec<usize>>>,
    /// 旋转阈值
    idle_threshold: usize,
    /// 标记为不可用的模型索引集合
    disabled_models: Arc<std::sync::Mutex<std::collections::HashSet<usize>>>,
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
        let count = clients.len();
        Self {
            clients,
            active: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            idle_counts: Arc::new(std::sync::Mutex::new(vec![0; count])),
            idle_threshold: 5, // 默认阈值
            disabled_models: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        }
    }

    /// 设置 idle 旋转阈值
    pub fn with_idle_threshold(mut self, threshold: usize) -> Self {
        self.idle_threshold = threshold;
        self
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

    /// 记录当前模型的 idle 行为，自动切换到下一个模型
    ///
    /// 如果当前模型连续 idle 达到阈值，则标记为不可用并切换。
    /// 返回 true 表示发生了切换，false 表示未达到阈值。
    pub fn record_idle(&self) -> bool {
        let current_idx = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let mut idle_counts = self.idle_counts.lock().unwrap();
        let mut disabled = self.disabled_models.lock().unwrap();

        // 增加当前模型的 idle 计数
        idle_counts[current_idx] += 1;
        let count = idle_counts[current_idx];

        if count >= self.idle_threshold {
            // 标记当前模型为不可用
            disabled.insert(current_idx);
            tracing::warn!(
                "LLM 模型 #{} 连续 idle {} 次，达到阈值 {}，标记为不可用",
                current_idx,
                count,
                self.idle_threshold
            );

            // 切换到下一个可用模型
            drop(disabled);
            self.rotate_to_next_available();
            return true;
        }

        false
    }

    /// 切换到下一个可用模型
    ///
    /// 跳过已标记为不可用的模型。如果所有模型都不可用，则保持当前状态。
    fn rotate_to_next_available(&self) {
        let start = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let disabled = self.disabled_models.lock().unwrap();

        for offset in 1..=self.clients.len() {
            let idx = (start + offset) % self.clients.len();
            if !disabled.contains(&idx) {
                let old = self.active.load(std::sync::atomic::Ordering::Relaxed);
                self.active.store(idx, std::sync::atomic::Ordering::Relaxed);
                tracing::warn!("LLM idle 旋转：模型 #{} → #{} (跳过不可用模型)", old, idx);
                return;
            }
        }

        tracing::error!("所有 LLM 模型都已标记为不可用，保持当前模型");
    }

    /// 重置当前模型的 idle 计数（当模型返回非 idle 结果时调用）
    pub fn reset_idle_count(&self) {
        let current_idx = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let mut idle_counts = self.idle_counts.lock().unwrap();
        let old_count = idle_counts[current_idx];
        if old_count > 0 {
            idle_counts[current_idx] = 0;
            tracing::debug!("LLM 模型 #{} idle 计数重置: {} → 0", current_idx, old_count);
        }
    }

    /// 获取当前活跃客户端
    fn active_client(&self) -> Arc<dyn LlmClient> {
        let idx = self.active.load(std::sync::atomic::Ordering::Relaxed);
        self.clients[idx.min(self.clients.len() - 1)].clone()
    }

    /// 判断错误是否应触发 fallback
    ///
    /// 匹配条件：
    /// - HTTP 404 (model_not_found / 模型不存在或无权限)
    /// - HTTP 403 (AllocationQuota / 额度耗尽)
    /// - HTTP 429 (Rate limit)
    /// - HTTP 400 (does not support http call / 模型不支持HTTP调用)
    /// - 空响应（模型返回 null/空内容）
    fn should_fallback(error: &anyhow::Error) -> bool {
        let msg = format!("{:#}", error);

        // Prompt 超长是确定性的，重试/fallback 无意义（所有模型都可能超长）
        if msg.contains("exceeds max context window")
            || msg.contains("Prompt too long")
            || msg.contains("context_length_exceeded")
            || msg.contains("maximum context length")
        {
            return false;
        }

        // HTTP 状态码匹配（直接来自 API 响应）
        msg.contains("LLM API error 404")
            || msg.contains("LLM API error 403")
            || msg.contains("LLM API error 429")
            || msg.contains("LLM streaming API error 404")
            || msg.contains("LLM streaming API error 403")
            || msg.contains("LLM streaming API error 429")
            // 400 Bad Request：模型能力不匹配（如 "only support stream mode"）
            || (msg.contains("LLM API error 400") && !msg.contains("Prompt too long"))
            || (msg.contains("LLM streaming API error 400") && !msg.contains("Prompt too long"))
            // 额度耗尽关键词
            || msg.contains("AllocationQuota")
            // 连接/请求失败（.context() 包装后的前缀）
            || msg.contains("Failed to send request to LLM API")
            || msg.contains("error sending request for url")
            // 空响应（MiniMax 等模型偶尔返回 content=null）
            || msg.contains("response content is empty")
            // DashScope "does not support http call"（开发测试用）
            || msg.contains("does not support http call")
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
                    let err_msg = format!("{:#}", &e);
                    if err_msg.contains("LLM API error 400") && !err_msg.contains("Prompt too long")
                    {
                        tracing::warn!(
                            "提示: 模型可能不支持 non-streaming，建议在 agent.yaml 中设置 prefer_stream: true"
                        );
                    }
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

    /// 流式调用的 fallback 逻辑
    ///
    /// 连接阶段失败（如 403/超时）自动切换到下一个 provider。
    /// 流中途失败直接返回 Err（无法中途切换）。
    /// 返回 (stream, provider_name, model_name)
    async fn call_streaming_with_fallback<F, Fut>(
        &self,
        f: F,
    ) -> Result<(super::streaming::LlmStream, String, String)>
    where
        F: Fn(Arc<dyn LlmClient>) -> Fut,
        Fut: std::future::Future<Output = Result<super::streaming::LlmStream>>,
    {
        let start = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let mut last_err = None;

        for offset in 0..self.clients.len() {
            let idx = (start + offset) % self.clients.len();
            let client = self.clients[idx].clone();

            match f(client.clone()).await {
                Ok(stream) => {
                    if offset > 0 {
                        tracing::warn!(
                            "LLM streaming fallback 成功：切换到客户端 #{} (主用 #{}）",
                            idx,
                            start
                        );
                        self.active.store(idx, std::sync::atomic::Ordering::Relaxed);
                    }
                    return Ok((stream, client.provider_name(), client.model_name()));
                }
                Err(e) => {
                    let should = Self::should_fallback(&e);
                    tracing::warn!(
                        "LLM streaming 客户端 #{} 失败 (fallback={}: {}",
                        idx,
                        should,
                        e
                    );
                    if !should {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("所有 LLM 客户端均失败")))
    }
}

#[allow(private_interfaces)]
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

    async fn send_chat_exchange(
        &self,
        messages: Vec<super::openai_types::ChatMessage>,
        tools: Option<&[super::tool_types::ToolDefinition]>,
        config: super::openai_types::ChatExchangeConfig,
    ) -> Result<super::openai_types::ChatExchangeResponse> {
        // NOTE: 当前 tool loop 路径不经过此处 — run_tool_loop 拿到的是底层
        // DirectLlmClient 的 &dyn LlmClient，fallback 在外层 complete_with_tools
        // 的 call_with_fallback 中以整体重试实现。
        self.active_client()
            .send_chat_exchange(messages, tools, config)
            .await
    }

    fn provider_name(&self) -> String {
        self.active_client().provider_name()
    }

    fn model_name(&self) -> String {
        self.active_client().model_name()
    }

    fn provider_info(&self) -> (super::direct_client::LlmProvider, String) {
        self.active_client().provider_info()
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

    async fn complete_with_conversation_and_tools(
        &self,
        system: &str,
        input: ConversationInput<'_>,
        tools: &[super::tool_types::ToolDefinition],
        executor: &dyn super::tool_types::ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let system = system.to_string();
        let turns = input.turns.to_vec();
        let current_prompt = input.current_prompt.to_string();
        let tools = tools.to_vec();
        let summary = input.summary.map(|s| s.to_string());
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let system = system.clone();
            let turns = turns.clone();
            let current_prompt = current_prompt.clone();
            let tools = tools.clone();
            let summary = summary.clone();
            async move {
                client
                    .complete_with_conversation_and_tools(
                        &system,
                        ConversationInput {
                            summary: summary.as_deref(),
                            turns: &turns,
                            current_prompt: &current_prompt,
                        },
                        &tools,
                        executor,
                        max_rounds,
                    )
                    .await
            }
        })
        .await
    }

    async fn complete_with_conversation(
        &self,
        system: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<String> {
        let system = system.to_string();
        let summary_owned = summary.map(|s| s.to_string());
        let turns = turns.to_vec();
        let current_prompt = current_prompt.to_string();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let system = system.clone();
            let summary = summary_owned.clone();
            let turns = turns.clone();
            let current_prompt = current_prompt.clone();
            async move {
                client
                    .complete_with_conversation(
                        &system,
                        summary.as_deref(),
                        &turns,
                        &current_prompt,
                    )
                    .await
            }
        })
        .await
    }

    fn complete_streaming<'a>(
        &'a self,
        system: &'a str,
        prompt: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<super::streaming::LlmStream>> + Send + 'a>,
    > {
        Box::pin(async move {
            use super::direct_client::LlmProvider;

            let system = system.to_string();
            let prompt = prompt.to_string();
            let (stream, provider_str, model) = self
                .call_streaming_with_fallback(move |client: Arc<dyn LlmClient>| {
                    let system = system.clone();
                    let prompt = prompt.clone();
                    async move { client.complete_streaming(&system, &prompt).await }
                })
                .await?;

            let provider = LlmProvider::parse(&provider_str).unwrap_or(LlmProvider::OpenClaw);
            let tracking_stream =
                super::streaming::UsageTrackingStream::new(stream, provider, model);
            Ok(tracking_stream.into_llm_stream())
        })
    }

    fn complete_conversation_streaming<'a>(
        &'a self,
        system: &'a str,
        summary: Option<&'a str>,
        turns: &'a [ConversationTurn],
        current_prompt: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<super::streaming::LlmStream>> + Send + 'a>,
    > {
        Box::pin(async move {
            use super::direct_client::LlmProvider;

            let system = system.to_string();
            let summary_owned = summary.map(|s| s.to_string());
            let turns = turns.to_vec();
            let current_prompt = current_prompt.to_string();
            let (stream, provider_str, model) = self
                .call_streaming_with_fallback(move |client: Arc<dyn LlmClient>| {
                    let system = system.clone();
                    let summary = summary_owned.clone();
                    let turns = turns.clone();
                    let current_prompt = current_prompt.clone();
                    async move {
                        client
                            .complete_conversation_streaming(
                                &system,
                                summary.as_deref(),
                                &turns,
                                &current_prompt,
                            )
                            .await
                    }
                })
                .await?;

            let provider = LlmProvider::parse(&provider_str).unwrap_or(LlmProvider::OpenClaw);
            let tracking_stream =
                super::streaming::UsageTrackingStream::new(stream, provider, model);
            Ok(tracking_stream.into_llm_stream())
        })
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

{"action_type": "进食", "action_data": {"item_id": "馒头"}, "speech_content": ""}"#;
        let result = strip_thinking_tags(input);
        assert!(
            result.contains(r#"{"action_type": "进食"#),
            "应保留 JSON，实际: {}",
            result
        );
        assert!(
            !result.contains("<think"),
            "应移除 think 标签，实际: {}",
            result
        );
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
        let input = "<think.../>\n考虑拾取馒头充饥\n\n{\"action_type\": \"drink\", \"action_data\": {\"item_id\": \"水\"}, \"speech_content\": \"\"}";
        let result = strip_thinking_tags(input);
        // 自闭合标签被移除，但思考文本仍在（非标签包裹无法剥离）
        assert!(!result.contains("<think"), "think 标签应被移除");
        assert!(result.contains("\"action_type\": \"drink\""), "JSON 应保留");
    }
}
