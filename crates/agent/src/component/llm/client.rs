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
    pub reasoning_content: Option<String>,
}

/// 对话输入参数（用于减少函数参数数量）
#[derive(Debug, Clone)]
pub struct ConversationInput<'a> {
    /// 半静态内容（actions + skills），变更频率低
    pub semi_static: &'a str,
    /// 旧轮次压缩摘要（独立 system message，仅 compaction 时变化）
    pub summary: Option<&'a str>,
    /// 保留的近期完整轮次
    pub turns: &'a [ConversationTurn],
    /// 当前请求的 prompt
    pub current_prompt: &'a str,
}

/// 构建对话消息列表（system + semi-static + summary + history + current tick）
///
/// 三区域分区：system（persona）→ semi-static（actions/skills）→ summary（压缩摘要）。
///
/// **通用逻辑 — 不针对任何 provider 特化。** OpenAI Chat Completions 规范对连续
/// 多个 `role: "system"` 消息的语义未定义，部分严格实现（如 sensenova）会直接
/// 拒绝返回 400。模型视角下 `[sys:A][sys:B][user:Q]` 与 `[sys:A\n\nB][user:Q]`
/// 信息量等价，合并是更安全且无损的默认。
pub fn build_conversation_messages(
    system: &str,
    semi_static: &str,
    summary: Option<&str>,
    turns: &[ConversationTurn],
    current_tick_message: &str,
) -> Vec<super::openai_types::ChatMessage> {
    use super::openai_types::ChatMessage;

    // 合并所有 system 段为单个 system message（通用兼容处理，无 provider 特化）
    let mut combined_system = String::with_capacity(system.len() + semi_static.len() + 64);
    combined_system.push_str(system);
    if !semi_static.is_empty() {
        combined_system.push_str("\n\n");
        combined_system.push_str(semi_static);
    }
    if let Some(s) = summary {
        combined_system.push_str("\n\n## 对话历史摘要\n");
        combined_system.push_str(s);
    }

    let mut messages = vec![ChatMessage::system(&combined_system)];
    for turn in turns {
        messages.push(ChatMessage::user(&turn.user));
        messages.push(ChatMessage::assistant_with_reasoning(
            &turn.assistant,
            turn.reasoning_content.clone(),
        ));
    }
    messages.push(ChatMessage::user(current_tick_message));
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

    /// 获取模型的上下文窗口大小（tokens）
    fn context_window_tokens(&self) -> u32 {
        32000
    }

    /// 截断重试时的 max_tokens 基线
    ///
    /// per-call `ChatExchangeConfig.max_tokens` 为 None 时,retry 翻倍以此为起点。
    /// `DirectLlmClient` 覆盖为 `self.config.max_tokens`(沿用全局配置)。
    /// 默认 = `DEFAULT_LLM_MAX_TOKENS / 2`(基线为输出预算一半,合理起点)。
    fn retry_max_tokens_baseline(&self) -> u32 {
        crate::config::DEFAULT_LLM_MAX_TOKENS / 2
    }

    /// 截断重试时 max_tokens 翻倍的上限
    ///
    /// `DirectLlmClient` 覆盖为 `self.config.context_window_tokens`。
    /// 默认 = `DEFAULT_LLM_MAX_TOKENS * 4`(4 倍预算,合理上限)。
    fn retry_max_tokens_ceiling(&self) -> u32 {
        crate::config::DEFAULT_LLM_MAX_TOKENS * 4
    }

    /// 当前 LLM 客户端使用的温度
    ///
    /// per-call config 构造时使用此值填充,避免调用方硬编码。
    fn temperature(&self) -> f32 {
        0.7
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

    /// 取回最近一次 LLM 调用的 reasoning_content（DeepSeek 等模型需要回传多轮对话）
    fn take_last_reasoning_content(&self) -> Option<String> {
        None
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
        let _ = (&input.semi_static, &input.summary, input.turns);
        self.complete_with_tools(system, input.current_prompt, tools, executor, max_rounds)
            .await
    }

    /// 使用对话历史完成调用（长窗口）
    ///
    /// `semi_static` 为半静态内容（actions + skills，变更频率低）。
    /// `summary` 为旧轮次的压缩摘要。
    /// `turns` 为保留的近期完整轮次。
    /// `current_prompt` 为当前 tick 的用户输入。
    ///
    /// 默认实现退化为 system + current_prompt（不使用历史）。
    async fn complete_with_conversation(
        &self,
        system: &str,
        semi_static: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<String> {
        let _ = (semi_static, summary, turns);
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
        semi_static: &'a str,
        summary: Option<&'a str>,
        turns: &'a [ConversationTurn],
        current_prompt: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<super::streaming::LlmStream>> + Send + 'a>,
    > {
        Box::pin(async move {
            let result = self
                .complete_with_conversation(system, semi_static, summary, turns, current_prompt)
                .await?;
            let stream = futures_util::stream::once(async move {
                Ok(super::streaming::StreamChunk::Delta(result))
            });
            let boxed: super::streaming::LlmStream = Box::pin(stream);
            Ok(boxed)
        })
    }
}

/// JSON 结构化调用结果（含 reasoning_content）
pub struct CompleteJsonResult<T> {
    pub value: T,
    pub reasoning_content: Option<String>,
}

/// LlmClient 扩展 Trait
///
/// 提供 complete_json 等辅助方法
#[async_trait]
pub trait LlmClientExt: LlmClient {
    /// 完成一次结构化输出调用（JSON 模式）
    async fn complete_json<T: DeserializeOwned + Send>(&self, prompt: &str) -> Result<T>;

    /// 完成一次结构化输出调用（JSON 模式，per-call config 覆盖 temperature 等）
    async fn complete_json_with_config<T: DeserializeOwned + Send>(
        &self,
        prompt: &str,
        config: super::openai_types::ChatExchangeConfig,
    ) -> Result<T>;

    /// 完成一次结构化输出调用，遇截断时自动扩大 max_tokens 重试
    async fn complete_json_with_config_and_retry<T: DeserializeOwned + Send>(
        &self,
        prompt: &str,
        mut config: super::openai_types::ChatExchangeConfig,
        max_retries: usize,
    ) -> Result<T> {
        let baseline = self.retry_max_tokens_baseline();
        let ceiling = self.retry_max_tokens_ceiling();
        for attempt in 0..=max_retries {
            match self.complete_json_with_config::<T>(prompt, config.clone()).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if !is_truncation_error(&e) || attempt == max_retries {
                        return Err(e);
                    }
                    let new_max = (config.max_tokens.unwrap_or(baseline) * 2).min(ceiling);
                    tracing::warn!(
                        "[LLM retry] 截断检测 attempt={}, max_tokens {} -> {}",
                        attempt + 1,
                        config.max_tokens.unwrap_or(baseline),
                        new_max
                    );
                    config.max_tokens = Some(new_max);
                }
            }
        }
        unreachable!()
    }

    /// 完成一次结构化输出调用（遇截断自动重试），并返回 reasoning_content
    ///
    /// 与 `complete_json_with_config_and_retry` 唯一区别：保留最后一次
    /// attempt 的 `reasoning_content`（供调试 / 未来 NPC SFT 数据采集）。
    async fn complete_json_with_config_and_retry_extracted<T: DeserializeOwned + Send>(
        &self,
        prompt: &str,
        mut config: super::openai_types::ChatExchangeConfig,
        max_retries: usize,
    ) -> Result<CompleteJsonResult<T>> {
        let messages = vec![super::openai_types::ChatMessage::user(prompt)];
        let baseline = self.retry_max_tokens_baseline();
        let ceiling = self.retry_max_tokens_ceiling();
        for attempt in 0..=max_retries {
            let response = self
                .send_chat_exchange(messages.clone(), None, config.clone())
                .await?;
            let content = response.content.unwrap_or_default();
            match parse_json_response::<T>(&content) {
                Ok(value) => {
                    return Ok(CompleteJsonResult {
                        value,
                        reasoning_content: response.reasoning_content,
                    });
                }
                Err(e) => {
                    if !is_truncation_error(&e) || attempt == max_retries {
                        return Err(e);
                    }
                    let new_max = (config.max_tokens.unwrap_or(baseline) * 2).min(ceiling);
                    tracing::warn!(
                        "[LLM retry] 截断检测 attempt={}, max_tokens {} -> {}",
                        attempt + 1,
                        config.max_tokens.unwrap_or(baseline),
                        new_max
                    );
                    config.max_tokens = Some(new_max);
                }
            }
        }
        unreachable!()
    }

    /// 完成一次结构化输出调用（system + user 分离，遇截断自动重试），并返回 reasoning_content
    ///
    /// 与 `complete_json_with_config_and_retry_extracted` 区别: 保留 system role 分离,
    /// 用于 ReflectorSoul 等需要明确角色指令的场景。
    async fn complete_json_with_system_and_retry_extracted<T: DeserializeOwned + Send>(
        &self,
        system: &str,
        prompt: &str,
        mut config: super::openai_types::ChatExchangeConfig,
        max_retries: usize,
    ) -> Result<CompleteJsonResult<T>> {
        let messages = vec![
            super::openai_types::ChatMessage::system(system),
            super::openai_types::ChatMessage::user(prompt),
        ];
        let baseline = self.retry_max_tokens_baseline();
        let ceiling = self.retry_max_tokens_ceiling();
        for attempt in 0..=max_retries {
            let response = self
                .send_chat_exchange(messages.clone(), None, config.clone())
                .await?;
            let content = response.content.unwrap_or_default();
            match parse_json_response::<T>(&content) {
                Ok(value) => {
                    return Ok(CompleteJsonResult {
                        value,
                        reasoning_content: response.reasoning_content,
                    });
                }
                Err(e) => {
                    if !is_truncation_error(&e) || attempt == max_retries {
                        return Err(e);
                    }
                    let new_max = (config.max_tokens.unwrap_or(baseline) * 2).min(ceiling);
                    tracing::warn!(
                        "[LLM retry] 截断检测 attempt={}, max_tokens {} -> {}",
                        attempt + 1,
                        config.max_tokens.unwrap_or(baseline),
                        new_max
                    );
                    config.max_tokens = Some(new_max);
                }
            }
        }
        unreachable!()
    }

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
        semi_static: &str,
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
        semi_static: &str,
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
    // 匹配配对标签: <think_tag>...</think_tag>, <think attrs>...</think attrs>,
    // <reasoning>...</reasoning>, <thought>...</thought>,
    // <minimax:tool_call>...</minimax:tool_call>（MiniMax 专有 XML tool_call 格式）
    let paired_re = regex::Regex::new(
        r"(?is)<(?:think_tag|think|reasoning|thought|minimax:tool_call)[^>]*>.*?</(?:think_tag|think|reasoning|thought|minimax:tool_call)[^>]*>"
    ).expect("static regex is valid");

    let cleaned = paired_re.replace_all(response, "").to_string();

    // 处理自闭合标签: <think/>, <think />, <think.../>, <think length="123"/>
    let self_closing_re = regex::Regex::new(
        r"(?i)<(?:think_tag|think|reasoning|thought|minimax:tool_call)[^>]*/>\s*",
    )
    .expect("static regex is valid");
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

/// LLM JSON 单遍结构化修复
///
/// 合并归一化 + 括号平衡为单遍状态机，消除两阶段状态漂移。
///
/// 核心原理：JSON 语法是确定性的——字符串闭合引号后**必须**跟随结构字符
/// （, } ] :）或空白+结构字符。不满足此条件的引号为字符串内嵌内容 → 转义。
///
/// 处理范围（单遍完成）：
/// 1. 中文引号 "" → " （字符串外）
/// 2. 单行注释 // ... → 移除
/// 3. 字符串内嵌未转义 " → 转义为 \"（key/value 上下文分别判定）
/// 4. 括号 {}[] 深度追踪 + 自动补全
/// 5. 尾部逗号/引号清理
///
/// 引号闭合判定规则（JSON 语法确定性）：
/// - key 闭合引号后：, } ] : 均合法（key 总是跟 : 配对）
/// - value 闭合引号后：仅 , } ] 合法（value 不跟 :）
///   通过 expect_key/in_key 追踪 key vs value 上下文。
///
/// 已知限制：无法区分「key 缺失闭合引号」和「值内嵌引号」，
/// 如 "appearance: "text" 会被整体视为一个 key 字符串。
fn repair_llm_json(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut result = String::with_capacity(input.len() + 32);
    let mut i = 0;

    // 状态
    let mut in_string = false;
    // key vs value 上下文：{ 或 , 后期望 key，: 后期望 value
    let mut expect_key = false;
    let mut in_key = false;
    // 括号栈：追踪 {} [] 嵌套（同时用于平衡补全）
    let mut bracket_stack: Vec<char> = Vec::new();

    while i < len {
        let c = chars[i];

        if in_string {
            // 转义序列：原样透传
            if c == '\\' && i + 1 < len {
                result.push('\\');
                i += 1;
                result.push(chars[i]);
                i += 1;
                continue;
            }

            // 遇到 " 或中文右引号：判断是字符串闭合还是值内未转义引号
            if c == '"' || c == '\u{201d}' {
                // 跳过空白，找到下一个有意义的字符
                let mut j = i + 1;
                while j < len && matches!(chars[j], ' ' | '\t' | '\n' | '\r') {
                    j += 1;
                }
                let next_meaningful = if j < len { Some(chars[j]) } else { None };

                // 闭合判定规则（基于 JSON 语法确定性规则）：
                // - key 闭合引号后必须跟 : → , } ] : 均合法
                // - value 闭合引号后必须跟 , } ] → 仅 , } ] 合法
                let is_closing = if in_key {
                    next_meaningful.is_none_or(|ch| matches!(ch, ',' | '}' | ']' | ':'))
                } else {
                    next_meaningful.is_none_or(|ch| matches!(ch, ',' | '}' | ']'))
                };

                if is_closing {
                    in_string = false;
                    in_key = false;
                    result.push('"');
                } else {
                    // 值内未转义引号 → 转义
                    result.push_str("\\\"");
                }
                i += 1;
                continue;
            }

            result.push(c);
            i += 1;
            continue;
        }

        // === 结构上下文 ===
        match c {
            '"' => {
                in_string = true;
                in_key = expect_key;
                result.push('"');
            }
            '\u{201c}' => {
                // 中文左引号 → ASCII 开引号
                in_string = true;
                in_key = expect_key;
                result.push('"');
            }
            '\u{201d}' => {
                // 中文右引号 → ASCII 闭引号
                in_string = false;
                in_key = false;
                result.push('"');
            }
            '{' => {
                bracket_stack.push('{');
                expect_key = true;
                result.push('{');
            }
            '}' => {
                // 移除尾逗号：, } → }
                if result.ends_with(',') {
                    result.pop();
                    result = result.trim_end().to_string();
                }
                // 括号平衡：弹出到匹配的 {
                if bracket_stack.is_empty() {
                    i += 1;
                    continue;
                }
                while let Some(&top) = bracket_stack.last() {
                    if top == '{' {
                        bracket_stack.pop();
                        break;
                    }
                    bracket_stack.pop();
                    result.push(']');
                }
                result.push('}');
                expect_key = false;

                // }, { 模式：连续对象间自动补全中间缺失的 }
                let mut peek = i + 1;
                while peek < len && matches!(chars[peek], ' ' | '\t' | '\n' | '\r') {
                    peek += 1;
                }
                if peek < len && chars[peek] == ',' {
                    peek += 1;
                    while peek < len && matches!(chars[peek], ' ' | '\t' | '\n' | '\r') {
                        peek += 1;
                    }
                    if peek < len && chars[peek] == '{' {
                        while bracket_stack.last() == Some(&'{') {
                            bracket_stack.pop();
                            result.push('}');
                        }
                    }
                }
            }
            '[' => {
                bracket_stack.push('[');
                expect_key = false;
                result.push('[');
            }
            ']' => {
                // 移除尾逗号：, ] → ]
                if result.ends_with(',') {
                    result.pop();
                    result = result.trim_end().to_string();
                }
                if bracket_stack.is_empty() {
                    i += 1;
                    continue;
                }
                while let Some(&top) = bracket_stack.last() {
                    if top == '[' {
                        bracket_stack.pop();
                        break;
                    }
                    bracket_stack.pop();
                    result.push('}');
                }
                result.push(']');
            }
            ':' => {
                expect_key = false;
                result.push(':');
            }
            ',' => {
                // 数组内 , 后不期望 key，对象内 , 后期望 key
                expect_key = bracket_stack.last() == Some(&'{');
                result.push(',');
            }
            // 单行注释 → 移除
            '/' if i + 1 < len && chars[i + 1] == '/' => {
                i += 2;
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            _ => {
                result.push(c);
            }
        }
        i += 1;
    }

    // 闭合未关闭的字符串
    if in_string {
        result.push('"');
    }

    // 尾部清理
    let mut fixed = result.trim_end().to_string();

    // 移除数组/对象闭合后的多余引号：]" → ], }" → }
    let bytes = fixed.as_bytes();
    let blen = bytes.len();
    if blen >= 2 {
        let last = bytes[blen - 1];
        let prev = bytes[blen - 2];
        if last == b'"' && (prev == b']' || prev == b'}') {
            fixed.pop();
        }
    }

    // 移除尾部逗号
    while let Some(last) = fixed.chars().last() {
        if last == ',' {
            fixed.pop();
            fixed = fixed.trim_end().to_string();
        } else {
            break;
        }
    }

    // 补全剩余未闭合的括号
    while let Some(top) = bracket_stack.pop() {
        fixed.push(if top == '{' { '}' } else { ']' });
    }

    fixed
}

/// 判断 LLM 错误是否由响应截断引起（用于触发 retry）
fn is_truncation_error(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        let s = c.to_string();
        s.contains("EOF while parsing")
            || s.contains("unexpected end of input")
            || s.contains("response body is not valid UTF-8")
    })
}

/// 解析 LLM 响应为结构化类型（单遍修复 → serde 解析）
fn parse_json_response<D: DeserializeOwned + Send>(response: &str) -> Result<D> {
    let raw_json = extract_json_str(response);
    let json_str = repair_llm_json(&raw_json);

    // 直接解析
    if let Ok(parsed) = serde_json::from_str::<D>(&json_str) {
        return Ok(parsed);
    }

    // 解析失败，输出诊断信息
    let parse_err = match serde_json::from_str::<D>(&json_str) {
        Ok(_) => unreachable!(),
        Err(e) => e,
    };
    let error_line = parse_err.line();
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
        error_type = ?parse_err.classify(),
        error_msg = %parse_err,
        line = error_line,
        column = parse_err.column(),
        json_len = json_str.len(),
        "\n{error_snippet}\n--- Full JSON ---\n{json_str}"
    );
    Err(parse_err.into())
}

#[async_trait]
impl<T: LlmClient + ?Sized> LlmClientExt for T {
    async fn complete_json<D: DeserializeOwned + Send>(&self, prompt: &str) -> Result<D> {
        let response = self.complete(prompt).await?;
        parse_json_response::<D>(&response)
    }

    async fn complete_json_with_config<D: DeserializeOwned + Send>(
        &self,
        prompt: &str,
        config: super::openai_types::ChatExchangeConfig,
    ) -> Result<D> {
        let messages = vec![super::openai_types::ChatMessage::user(prompt)];
        let response = self.send_chat_exchange(messages, None, config).await?;
        let content = response.content.unwrap_or_default();
        parse_json_response::<D>(&content)
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
        semi_static: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<D> {
        let response = self
            .complete_with_conversation(system, semi_static, summary, turns, current_prompt)
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

        let stats = acc.token_stats();
        let pt = stats.prompt_tokens;
        let ct = stats.completion_tokens;
        if pt > 0 || ct > 0 {
            tracing::debug!(
                "Streaming JSON token usage: prompt={}, completion={}, real={}",
                pt,
                ct,
                stats.has_real_usage
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
        semi_static: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<D> {
        use futures_util::StreamExt;

        let stream = self
            .complete_conversation_streaming(system, semi_static, summary, turns, current_prompt)
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

        let stats = acc.token_stats();
        let pt = stats.prompt_tokens;
        let ct = stats.completion_tokens;
        if pt > 0 || ct > 0 {
            tracing::debug!(
                "Streaming JSON conv token usage: prompt={}, completion={}, real={}",
                pt,
                ct,
                stats.has_real_usage
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

/// 429 disable 的恢复间隔（1 小时）
const RATE_LIMIT_BACKOFF_SECS: u64 = 3600;

// ============================================================================
// 共享 Circuit-Breaker
// ============================================================================
//
// 修复 FINDING-002: 此前 `disabled_models` 仅存在于 `FallbackLlmClient`，
// 但 `run_tool_loop` 内部 `send_chat_exchange` 直接打到 `DirectLlmClient`，
// 完全绕过该表，导致 sensenova 抖动一次就被放大成 566 次 400。
//
// 抽 `SharedBreaker` 后，FallbackLlmClient 和 DirectLlmClient 共享同一份
// "已禁用 provider/model" 表，任意入口（fallback / tool_loop）都能命中。
// key = `"{provider}/{model}"`，多个 agent 共享同一 provider/model 时
// 也会一起退避（避免雪崩式打 sensenova）。

/// 共享 circuit-breaker 状态。
#[derive(Default)]
pub struct SharedBreaker {
    /// key: `"{provider}/{model}"`; value: 禁用开始时间
    disabled: std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

impl std::fmt::Debug for SharedBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let disabled = self.disabled.lock().expect("lock poisoned");
        f.debug_struct("SharedBreaker")
            .field("disabled_keys", &disabled.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SharedBreaker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 查询 key 是否被禁用。返回 `Some(remaining_secs)` 表示仍在冷却，
    /// `None` 表示可用（不在表内或已过期）。
    pub fn is_disabled(&self, key: &str) -> Option<u64> {
        let mut disabled = self.disabled.lock().expect("lock poisoned");
        // 清理过期项
        let now = std::time::Instant::now();
        disabled.retain(|_, ts| now.duration_since(*ts).as_secs() < RATE_LIMIT_BACKOFF_SECS);
        disabled.get(key).map(|ts| {
            let elapsed = now.duration_since(*ts).as_secs();
            RATE_LIMIT_BACKOFF_SECS.saturating_sub(elapsed)
        })
    }

    /// 标记 key 禁用
    pub fn disable(&self, key: String) {
        let mut disabled = self.disabled.lock().expect("lock poisoned");
        disabled.insert(key, std::time::Instant::now());
    }
}

// ============================================================================
// 统一错误分类 — 三处消费者共享同一份分类逻辑
// ============================================================================

/// LLM 调用错误的处理策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorAction {
    /// 放弃 — 错误不可恢复，无需重试（auth、context 超长等）
    GiveUp,
    /// 切换到下一个 model/provider（403 额度不足、404 模型不存在等）
    Fallback,
    /// 禁用当前 model 后切换到下一个（429 限流、provider 炸了等）
    FallbackAndDisable,
    /// 重试（网络瞬时故障、连接超时等）
    Retry,
}

/// 根据 error 内容分类 LLM 调用错误
///
/// 返回 `(ErrorAction, &'static str)`，第二元素为 disable_model 的简短原因。
///
/// 三处消费者：
/// - `call_with_fallback + call_streaming_with_fallback`:
///   `Fallback | FallbackAndDisable | Retry` → 继续轮询下一模型
///   `FallbackAndDisable` → disable_model + rotate
/// - `decision.rs` retry loop: `GiveUp | Fallback | FallbackAndDisable` → break
pub fn classify_llm_error(error: &anyhow::Error) -> (ErrorAction, &'static str) {
    let msg = format!("{:#}", error);

    // ── Permanent: 确定性的，重试/fallback 无意义 ──────────────
    if msg.contains("exceeds max context window")
        || msg.contains("Prompt too long")
        || msg.contains("context_length_exceeded")
        || msg.contains("maximum context length")
    {
        return (ErrorAction::GiveUp, "context_too_long");
    }

    // ── Config/Model: 换模型可能解决 ────────────────────────────
    if msg.contains("LLM API error 404")
        || msg.contains("LLM streaming API error 404")
        || msg.contains("AllocationQuota")
    {
        return (ErrorAction::Fallback, "model_not_found_or_quota");
    }

    // 403 也可能是配额问题
    if msg.contains("LLM API error 403") || msg.contains("LLM streaming API error 403") {
        return (ErrorAction::Fallback, "forbidden_or_quota");
    }

    // ── Rate limit: 禁用模型（1h 冷却），避免 OOM ──────────────
    if msg.contains("429")
        || msg.contains("rate_limit")
        || msg.contains("Too Many Requests")
        || msg.contains("LLM API error 429")
        || msg.contains("LLM streaming API error 429")
    {
        return (ErrorAction::FallbackAndDisable, "rate_limit");
    }

    // ── Provider 内部错误: 如 sensenova 把 503 包装成 400 internal_server_error ──
    if msg.contains("internal_server_error") {
        return (ErrorAction::FallbackAndDisable, "internal_server_error");
    }

    // ── 空响应: 模型偶尔返回 null ────────────────────────────────
    if msg.contains("response content is empty") {
        return (ErrorAction::FallbackAndDisable, "empty_response");
    }

    // ── 400 Bad Request: 模型能力不匹配（如 "only support stream mode"）─
    if msg.contains("LLM API error 400") || msg.contains("LLM streaming API error 400") {
        return (ErrorAction::Fallback, "bad_request");
    }

    // ── 连接/请求失败: 网络瞬时故障 ─────────────────────────────
    if msg.contains("Failed to send request to LLM API")
        || msg.contains("error sending request for url")
        || msg.contains("does not support http call")
    {
        return (ErrorAction::Retry, "connection_failed");
    }

    // 未知错误 — 保守起见，允许 fallback
    (ErrorAction::Retry, "unknown_error")
}

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
    /// 标记为不可用的模型索引 + disable 时间戳
    disabled_models: Arc<std::sync::Mutex<std::collections::HashMap<usize, std::time::Instant>>>,
    /// 共享 circuit-breaker：写入时同步到下层 DirectLlmClient，
    /// 使 `run_tool_loop` 内部 `send_chat_exchange` 也能命中。
    shared_breaker: Arc<SharedBreaker>,
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
            disabled_models: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            shared_breaker: Arc::new(SharedBreaker::new()),
        }
    }

    /// 设置 idle 旋转阈值
    pub fn with_idle_threshold(mut self, threshold: usize) -> Self {
        self.idle_threshold = threshold;
        self
    }

    /// 注入共享 circuit-breaker（由 build_fallback_client 调用，
    /// 必须与下层 DirectLlmClient 持有的 Arc 指向同一实例）
    pub fn with_shared_breaker(mut self, breaker: Arc<SharedBreaker>) -> Self {
        self.shared_breaker = breaker;
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
        let mut idle_counts = self.idle_counts.lock().expect("lock poisoned");
        let mut disabled = self.disabled_models.lock().expect("lock poisoned");

        // 增加当前模型的 idle 计数
        idle_counts[current_idx] += 1;
        let count = idle_counts[current_idx];

        if count >= self.idle_threshold {
            // 标记当前模型为不可用
            disabled.insert(current_idx, std::time::Instant::now());
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
        let disabled = self.disabled_models.lock().expect("lock poisoned");

        for offset in 1..=self.clients.len() {
            let idx = (start + offset) % self.clients.len();
            if !disabled.contains_key(&idx) {
                let old = self.active.load(std::sync::atomic::Ordering::Relaxed);
                self.active.store(idx, std::sync::atomic::Ordering::Relaxed);
                tracing::warn!("LLM idle 旋转：模型 #{} → #{} (跳过不可用模型)", old, idx);
                return;
            }
        }

        tracing::error!("所有 LLM 模型都已标记为不可用，保持当前模型");
    }

    /// 标记指定模型为不可用（429 circuit breaker 等）
    fn disable_model(&self, idx: usize, reason: &str) {
        let mut disabled = self.disabled_models.lock().expect("lock poisoned");
        if disabled.insert(idx, std::time::Instant::now()).is_none() {
            // 同步写入共享 breaker：key = "{provider}/{model}"，
            // 使下层 DirectLlmClient 在 tool_loop 内部 send_chat_exchange 时也能命中。
            let key = format!(
                "{}/{}",
                self.clients[idx].provider_name(),
                self.clients[idx].model_name()
            );
            self.shared_breaker.disable(key);

            tracing::warn!(
                "LLM 模型 #{} 标记为不可用 (原因: {})，已禁用模型: {:?}",
                idx,
                reason,
                disabled.keys().collect::<Vec<_>>()
            );
            drop(disabled);
            self.rotate_to_next_available();
        }
    }

    fn reenable_expired(&self) {
        let mut disabled = self.disabled_models.lock().expect("lock poisoned");
        let now = std::time::Instant::now();
        let expired: Vec<usize> = disabled
            .iter()
            .filter(|&(_, ts)| now.duration_since(*ts).as_secs() >= RATE_LIMIT_BACKOFF_SECS)
            .map(|(&idx, _)| idx)
            .collect();

        for idx in &expired {
            disabled.remove(idx);
        }
        drop(disabled);

        if !expired.is_empty() {
            tracing::info!(
                "429 circuit breaker 恢复: 模型 {:?} 已重新激活 (冷却期 {}s 已过)",
                expired,
                RATE_LIMIT_BACKOFF_SECS
            );
        }
    }

    /// 重置当前模型的 idle 计数（当模型返回非 idle 结果时调用）
    pub fn reset_idle_count(&self) {
        let current_idx = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let mut idle_counts = self.idle_counts.lock().expect("lock poisoned");
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

    /// 执行带 fallback 的调用（返回类型由闭包决定）
    ///
    /// 策略：从 active index 开始，失败时尝试后续所有客户端。
    /// 一旦成功，sticky 到该客户端。`FallbackAndDisable` 会同步写 shared_breaker。
    async fn call_with_fallback<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: Fn(Arc<dyn LlmClient>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        self.reenable_expired();
        let start = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let mut last_err = None;

        for offset in 0..self.clients.len() {
            let idx = (start + offset) % self.clients.len();

            // 跳过已被 circuit breaker 禁用的模型（短锁，不跨 await）
            if self
                .disabled_models
                .lock()
                .expect("lock poisoned")
                .contains_key(&idx)
            {
                continue;
            }

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
                    let (action, reason) = classify_llm_error(&e);
                    let is_fallback = matches!(
                        action,
                        ErrorAction::Fallback
                            | ErrorAction::FallbackAndDisable
                            | ErrorAction::Retry
                    );
                    tracing::warn!("LLM 客户端 #{} 调用失败 (action={:?}): {}", idx, action, e);
                    if action == ErrorAction::FallbackAndDisable {
                        self.disable_model(idx, reason);
                    }
                    let err_msg = format!("{:#}", &e);
                    if err_msg.contains("LLM API error 400") && !err_msg.contains("Prompt too long")
                    {
                        tracing::warn!(
                            "提示: 模型可能不支持 non-streaming，建议在 agent.yaml 中设置 prefer_stream: true"
                        );
                    }
                    if !is_fallback {
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
        self.reenable_expired();
        let start = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let mut last_err = None;

        for offset in 0..self.clients.len() {
            let idx = (start + offset) % self.clients.len();

            // 跳过已被 circuit breaker 禁用的模型（短锁，不跨 await）
            if self
                .disabled_models
                .lock()
                .expect("lock poisoned")
                .contains_key(&idx)
            {
                continue;
            }

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
                    let (action, reason) = classify_llm_error(&e);
                    let is_fallback = matches!(
                        action,
                        ErrorAction::Fallback
                            | ErrorAction::FallbackAndDisable
                            | ErrorAction::Retry
                    );
                    tracing::warn!(
                        "LLM streaming 客户端 #{} 失败 (action={:?}): {}",
                        idx,
                        action,
                        e
                    );
                    if action == ErrorAction::FallbackAndDisable {
                        self.disable_model(idx, reason);
                    }
                    if !is_fallback {
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
        // 走 call_with_fallback：跳过已 disabled 的客户端，
        // FallbackAndDisable 时同步写 shared_breaker。
        // 此前直调 active_client 会在 tool_loop 内部绕过 circuit breaker。
        let tools_opt = tools.map(|t| t.to_vec());
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let messages = messages.clone();
            let tools_inner = tools_opt.clone();
            let config = config.clone();
            async move {
                client
                    .send_chat_exchange(messages, tools_inner.as_deref(), config)
                    .await
            }
        })
        .await
    }

    fn provider_name(&self) -> String {
        self.active_client().provider_name()
    }

    fn model_name(&self) -> String {
        self.active_client().model_name()
    }

    fn context_window_tokens(&self) -> u32 {
        self.active_client().context_window_tokens()
    }

    fn provider_info(&self) -> (super::direct_client::LlmProvider, String) {
        self.active_client().provider_info()
    }

    fn take_last_reasoning_content(&self) -> Option<String> {
        self.active_client().take_last_reasoning_content()
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
        let semi_static = input.semi_static.to_string();
        let summary_owned = input.summary.map(|s| s.to_string());
        let turns = input.turns.to_vec();
        let current_prompt = input.current_prompt.to_string();
        let tools = tools.to_vec();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let system = system.clone();
            let semi_static = semi_static.clone();
            let summary = summary_owned.clone();
            let turns = turns.clone();
            let current_prompt = current_prompt.clone();
            let tools = tools.clone();
            async move {
                client
                    .complete_with_conversation_and_tools(
                        &system,
                        ConversationInput {
                            semi_static: &semi_static,
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
        semi_static: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<String> {
        let system = system.to_string();
        let semi_static = semi_static.to_string();
        let summary_owned = summary.map(|s| s.to_string());
        let turns = turns.to_vec();
        let current_prompt = current_prompt.to_string();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let system = system.clone();
            let semi_static = semi_static.clone();
            let summary = summary_owned.clone();
            let turns = turns.clone();
            let current_prompt = current_prompt.clone();
            async move {
                client
                    .complete_with_conversation(
                        &system,
                        &semi_static,
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
            let prompt_chars = (system.len() + prompt.len()) as u64;
            let (stream, provider_str, model) = self
                .call_streaming_with_fallback(move |client: Arc<dyn LlmClient>| {
                    let system = system.clone();
                    let prompt = prompt.clone();
                    async move { client.complete_streaming(&system, &prompt).await }
                })
                .await?;

            let provider = LlmProvider::parse(&provider_str).unwrap_or(LlmProvider::OpenClaw);
            let tracking_stream =
                super::streaming::UsageTrackingStream::new(stream, provider, model, prompt_chars);
            Ok(tracking_stream.into_llm_stream())
        })
    }

    fn complete_conversation_streaming<'a>(
        &'a self,
        system: &'a str,
        semi_static: &'a str,
        summary: Option<&'a str>,
        turns: &'a [ConversationTurn],
        current_prompt: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<super::streaming::LlmStream>> + Send + 'a>,
    > {
        Box::pin(async move {
            use super::direct_client::LlmProvider;

            let system = system.to_string();
            let semi_static = semi_static.to_string();
            let summary_owned = summary.map(|s| s.to_string());
            let turns = turns.to_vec();
            let current_prompt = current_prompt.to_string();
            let prompt_chars = {
                let mut total = system.len();
                total += semi_static.len();
                if let Some(ref s) = summary_owned {
                    total += s.len();
                }
                for turn in &turns {
                    total += turn.user.len();
                    total += turn.assistant.len();
                }
                total += current_prompt.len();
                total as u64
            };
            let (stream, provider_str, model) = self
                .call_streaming_with_fallback(move |client: Arc<dyn LlmClient>| {
                    let system = system.clone();
                    let semi_static = semi_static.clone();
                    let summary = summary_owned.clone();
                    let turns = turns.clone();
                    let current_prompt = current_prompt.clone();
                    async move {
                        client
                            .complete_conversation_streaming(
                                &system,
                                &semi_static,
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
                super::streaming::UsageTrackingStream::new(stream, provider, model, prompt_chars);
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

    #[test]
    fn test_strip_minimax_think_paired_with_attrs() {
        // MiniMax M2.7 配对 think 标签：opening/closing 均含属性
        let input = r#"<think HTaming>分析角色特征...</think HTaming>{"name": "测试", "age": 25}"#;
        let result = strip_thinking_tags(input);
        assert!(
            !result.contains("<think"),
            "think tag should be removed: {}",
            result
        );
        assert!(
            !result.contains("HTaming"),
            "think content should be removed: {}",
            result
        );
        assert!(
            result.contains(r#""name": "测试""#),
            "JSON should be preserved: {}",
            result
        );
    }

    // ========================================================================
    // repair_llm_json tests
    // ========================================================================

    #[test]
    fn test_repair_valid_json_passthrough() {
        let input = r#"{"name": "test", "age": 25}"#;
        let result = repair_llm_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["name"], "test");
        assert_eq!(parsed["age"], 25);
    }

    #[test]
    fn test_repair_embedded_unescaped_quotes() {
        // 实际 LongCat-2.0-Preview 输出：identity 值内含未转义 ASCII "
        let input = r#"{"identity": "曾是江湖上赫赫有名的"追魂针"沈三"}"#;
        let result = repair_llm_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["identity"].as_str().unwrap().contains("追魂针"));
        assert!(parsed["identity"].as_str().unwrap().contains("沈三"));
    }

    #[test]
    fn test_repair_embedded_quotes_before_comma() {
        let input = r#"{"desc": "他说"完毕"后离开", "name": "test"}"#;
        let result = repair_llm_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["desc"].as_str().unwrap().contains("完毕"));
        assert_eq!(parsed["name"], "test");
    }

    #[test]
    fn test_repair_chinese_quotes_as_delimiters() {
        let input = "{\u{201c}name\u{201d}: \u{201c}test\u{201d}}";
        let result = repair_llm_json(input);
        assert!(
            serde_json::from_str::<serde_json::Value>(&result).is_ok(),
            "Chinese quotes should produce valid JSON, got: {}",
            result
        );
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["name"], "test");
    }

    #[test]
    fn test_repair_unbalanced_brackets() {
        let input = r#"{"a": {"b": 1"#;
        let result = repair_llm_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"]["b"], 1);
    }

    #[test]
    fn test_repair_trailing_comma() {
        let input = r#"{"a": 1,}"#;
        let result = repair_llm_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn test_repair_line_comment() {
        let input = "{\n  \"a\": 1 // comment\n}";
        let result = repair_llm_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn test_repair_string_at_end_of_input() {
        // 字符串值在输入末尾闭合，next_meaningful = None
        let input = r#"{"a": "test"}"#;
        let result = repair_llm_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], "test");
    }

    #[test]
    fn test_repair_unclosed_string() {
        let input = r#"{"a": "test"#;
        let result = repair_llm_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], "test");
    }

    #[test]
    fn test_repair_quote_before_colon_in_value() {
        // " 后跟 : 不应被误判为闭合（已从匹配集中移除 :）
        let input = r#"{"desc": "a:b"}"#;
        let result = repair_llm_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["desc"], "a:b");
    }

    // ========================================================================
    // SharedBreaker tests — 验证 disable / is_disabled / 自动清理过期项
    // ========================================================================

    #[test]
    fn test_shared_breaker_disable_and_query() {
        let breaker = SharedBreaker::new();
        // 初始：所有 key 都可用
        assert!(
            breaker
                .is_disabled("openai_compatible/sensenova-6.7-flash-lite")
                .is_none()
        );

        // 禁用后：返回剩余秒数
        breaker.disable("openai_compatible/sensenova-6.7-flash-lite".to_string());
        let remaining = breaker.is_disabled("openai_compatible/sensenova-6.7-flash-lite");
        assert!(remaining.is_some(), "禁用后应返回 Some(remaining)");
        let secs = remaining.unwrap();
        assert!(
            secs > 0 && secs <= RATE_LIMIT_BACKOFF_SECS,
            "剩余秒数应在 (0, {}] 区间",
            RATE_LIMIT_BACKOFF_SECS
        );

        // 其他 key 不受影响
        assert!(
            breaker
                .is_disabled("openai_compatible/other-model")
                .is_none()
        );
    }

    #[test]
    fn test_shared_breaker_overwrite_disable() {
        let breaker = SharedBreaker::new();
        let key = "openai_compatible/x".to_string();
        breaker.disable(key.clone());
        // 二次 disable 不应 panic
        breaker.disable(key);
        assert!(breaker.is_disabled("openai_compatible/x").is_some());
    }

    #[test]
    fn test_shared_breaker_default() {
        let breaker = SharedBreaker::default();
        assert!(breaker.is_disabled("any/key").is_none());
    }

    #[test]
    fn test_shared_breaker_is_send_sync() {
        // 编译期断言：SharedBreaker 必须能跨线程共享
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SharedBreaker>();
    }
}
