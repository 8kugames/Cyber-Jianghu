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
    strip_reasoning: bool,
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
            if strip_reasoning {
                None
            } else {
                turn.reasoning_content.clone()
            },
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
        32768
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
    /// 默认实现返回 UNKNOWN_MODEL_PLACEHOLDER（"unknown"）；
    /// 归一判定在 `component::llm::normalize_model_id`，两处必须同源，
    /// 否则改动占位符会让"未上报"识别静默失效。
    fn model_name(&self) -> String {
        super::UNKNOWN_MODEL_PLACEHOLDER.to_string()
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

    /// 取回最近一次 tool loop 的 tool call 日志
    fn take_last_tool_call_log(&self) -> Option<Vec<cyber_jianghu_protocol::EarthToolCall>> {
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

/// 共享的「send_chat_exchange + JSON 解析 + 截断翻倍重试」实现。
///
/// `complete_json_with_config_and_retry_extracted` 与
/// `complete_json_with_system_and_retry_extracted` 仅 messages 构造不同，
/// 重试循环在此唯一维护。
async fn send_exchange_json_with_retry<T: DeserializeOwned + Send, C: LlmClient + ?Sized>(
    client: &C,
    messages: Vec<super::openai_types::ChatMessage>,
    mut config: super::openai_types::ChatExchangeConfig,
    max_retries: usize,
) -> Result<CompleteJsonResult<T>> {
    let baseline = client.retry_max_tokens_baseline();
    let ceiling = client.retry_max_tokens_ceiling();
    for attempt in 0..=max_retries {
        let response = client
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
            match self
                .complete_json_with_config::<T>(prompt, config.clone())
                .await
            {
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
    /// attempt 的 `reasoning_content`（供调试）。
    async fn complete_json_with_config_and_retry_extracted<T: DeserializeOwned + Send>(
        &self,
        prompt: &str,
        config: super::openai_types::ChatExchangeConfig,
        max_retries: usize,
    ) -> Result<CompleteJsonResult<T>> {
        let messages = vec![super::openai_types::ChatMessage::user(prompt)];
        send_exchange_json_with_retry::<T, Self>(self, messages, config, max_retries).await
    }

    /// 完成一次结构化输出调用（system + user 分离，遇截断自动重试），并返回 reasoning_content
    ///
    /// 与 `complete_json_with_config_and_retry_extracted` 区别: 保留 system role 分离,
    /// 用于 ReflectorSoul 等需要明确角色指令的场景。
    async fn complete_json_with_system_and_retry_extracted<T: DeserializeOwned + Send>(
        &self,
        system: &str,
        prompt: &str,
        config: super::openai_types::ChatExchangeConfig,
        max_retries: usize,
    ) -> Result<CompleteJsonResult<T>> {
        let messages = vec![
            super::openai_types::ChatMessage::system(system),
            super::openai_types::ChatMessage::user(prompt),
        ];
        send_exchange_json_with_retry::<T, Self>(self, messages, config, max_retries).await
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

    /// 流式 JSON 消费共享实现：排空流 → usage 日志 → 截断委托重试 → 空检 → 解析。
    ///
    /// `complete_json_streaming` 与 `complete_json_streaming_with_conversation`
    /// 仅取流入口与 `tag` 不同，消费逻辑在此唯一维护。
    async fn drain_stream_parse_json<D: DeserializeOwned + Send>(
        &self,
        stream: super::streaming::LlmStream,
        retry_prompt: &str,
        tag: &str,
    ) -> Result<D> {
        use futures_util::StreamExt;

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
                "Streaming JSON {} token usage: prompt={}, completion={}, real={}",
                tag,
                pt,
                ct,
                stats.has_real_usage
            );
        }

        // 截断检测：finish_reason=length 且 JSON 不完整（在 into_parts 前执行）
        // 复用 complete_json_with_config_and_retry_extracted 机制
        // （当 prefer_stream=true 时，send_chat_exchange 内部走流式路径）
        if acc.is_truncated() && !json_complete {
            tracing::warn!(
                "[streaming] 截断检测: content_len={}, 委托重试机制(max_tokens翻倍)",
                acc.content().len(),
            );
            let chat_config = super::openai_types::ChatExchangeConfig {
                model: self.model_name(),
                temperature: self.temperature(),
                max_tokens: None,
                enable_thinking: None,
            };
            let extracted = self
                .complete_json_with_config_and_retry_extracted::<D>(retry_prompt, chat_config, 2)
                .await?;
            return Ok(extracted.value);
        }

        let (content, _, reasoning_content) = acc.into_parts();
        let json_str = if content.trim().is_empty() && !reasoning_content.trim().is_empty() {
            tracing::info!(
                "[streaming] content 为空，从 reasoning_content 提取 JSON (reasoning_len={})",
                reasoning_content.len()
            );
            &reasoning_content
        } else {
            &content
        };

        if json_str.trim().is_empty() {
            anyhow::bail!(
                "LLM API error: response content is empty (streaming_{}, prompt_tokens={}, completion_tokens={})",
                tag,
                pt,
                ct
            );
        }

        parse_json_response::<D>(json_str)
    }

    /// 流式完成结构化输出（system + user）
    ///
    /// 内部消费 SSE 流，累积文本，JSON 闭合后早期终止。
    async fn complete_json_streaming<T: DeserializeOwned + Send>(
        &self,
        system: &str,
        prompt: &str,
    ) -> Result<T> {
        let stream = self.complete_streaming(system, prompt).await?;
        self.drain_stream_parse_json::<T>(stream, prompt, "json")
            .await
    }

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
    ) -> Result<T> {
        let stream = self
            .complete_conversation_streaming(system, semi_static, summary, turns, current_prompt)
            .await?;
        self.drain_stream_parse_json::<T>(stream, current_prompt, "json_conv")
            .await
    }

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
}

mod fallback;
mod json_utils;

pub use fallback::{ErrorAction, FallbackLlmClient, SharedBreaker, classify_llm_error};
pub(super) use json_utils::normalize_double_braces;
use json_utils::{is_truncation_error, parse_json_response};
pub mod mock;

#[cfg(test)]
#[path = "client/client_tests.rs"]
mod tests;
