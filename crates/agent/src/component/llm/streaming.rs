// ============================================================================
// LLM SSE 流式响应处理
// ============================================================================
//
// 解析 OpenAI 兼容 API 的 SSE 流式响应：
// - 将 reqwest bytes_stream 解析为 StreamChunk 序列
// - StreamAccumulator 累积增量文本，支持 JSON 闭合后早期终止
// - 复用 client.rs 的 find_first_json_end 逻辑检测 JSON 完整性
// ============================================================================

use anyhow::Result;
use futures_util::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::openai_types::OpenAIStreamResponse;

// ============================================================================
// 公共类型
// ============================================================================

/// SSE 流的一个 chunk
#[derive(Debug, Clone)]
pub enum StreamChunk {
    /// 文本增量
    Delta(String),
    /// tool_calls 增量（SSE delta.tool_calls 中的单项）
    ToolCallDelta(super::tool_types::StreamToolCallDelta),
    /// 推理/思考内容增量（SenseNova、DeepSeek 等模型的 reasoning_content）
    ReasoningDelta(String),
    /// 流结束（含 token 用量）
    Done {
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    /// 流结束，但服务端未返回 usage（需要估算）
    DoneEstimation { completion_chars: u64 },
}

/// SSE 流类型
pub type LlmStream = Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>;

/// 包装流，自动记录 token 用量
///
/// 当流产出 `Done` 或 `DoneEstimation` chunk 时，自动调用 `record_token_usage`。
pub struct UsageTrackingStream {
    inner: LlmStream,
    provider: super::direct_client::LlmProvider,
    model: String,
    recorded: bool,
}

impl UsageTrackingStream {
    pub fn new(
        inner: LlmStream,
        provider: super::direct_client::LlmProvider,
        model: String,
    ) -> Self {
        Self {
            inner,
            provider,
            model,
            recorded: false,
        }
    }

    /// 转换为 `LlmStream` 类型
    pub fn into_llm_stream(self) -> LlmStream {
        Box::pin(self) as LlmStream
    }
}

impl Stream for UsageTrackingStream {
    type Item = Result<StreamChunk>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // 直接调用 Stream::poll_next
        let inner = &mut self.inner;
        match Pin::new(inner).poll_next(cx) {
            Poll::Ready(Some(result)) => {
                // 只在第一个 Done/DoneEstimation 时记录
                if !self.recorded {
                    if let Ok(StreamChunk::Done {
                        prompt_tokens,
                        completion_tokens,
                    }) = &result
                    {
                        self.recorded = true;
                        super::token_tracking::record_token_usage(
                            &self.provider,
                            &self.model,
                            *prompt_tokens,
                            *completion_tokens,
                        );
                        tracing::debug!(
                            "UsageTrackingStream recorded: provider={}, model={}, prompt={}, completion={}",
                            self.provider.as_str(),
                            self.model,
                            prompt_tokens,
                            completion_tokens
                        );
                    } else if let Ok(StreamChunk::DoneEstimation { completion_chars }) = &result {
                        self.recorded = true;
                        let est_tokens = (completion_chars / 3).max(1);
                        super::token_tracking::record_token_usage(
                            &self.provider,
                            &self.model,
                            0,
                            est_tokens,
                        );
                        tracing::debug!(
                            "UsageTrackingStream recorded (est): provider={}, model={}, completion_est={}",
                            self.provider.as_str(),
                            self.model,
                            est_tokens
                        );
                    }
                }
                Poll::Ready(Some(result))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

// ============================================================================
// 流累积器
// ============================================================================

/// 将 StreamChunk 累积为完整响应文本
pub struct StreamAccumulator {
    content: String,
    reasoning_content: String,
    prompt_tokens: u64,
    completion_tokens: u64,
    has_real_usage: bool,
    tool_call_acc: super::tool_types::StreamToolCallAccumulator,
    has_tool_calls: bool,
}

impl Default for StreamAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            reasoning_content: String::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
            has_real_usage: false,
            tool_call_acc: super::tool_types::StreamToolCallAccumulator::new(),
            has_tool_calls: false,
        }
    }

    /// 追加一个 chunk
    pub fn push(&mut self, chunk: StreamChunk) {
        match chunk {
            StreamChunk::Delta(text) => self.content.push_str(&text),
            StreamChunk::ReasoningDelta(text) => self.reasoning_content.push_str(&text),
            StreamChunk::ToolCallDelta(delta) => {
                self.has_tool_calls = true;
                self.tool_call_acc.push(&delta);
            }
            StreamChunk::Done {
                prompt_tokens,
                completion_tokens,
            } => {
                self.prompt_tokens = prompt_tokens;
                self.completion_tokens = completion_tokens;
                self.has_real_usage = true;
            }
            StreamChunk::DoneEstimation { completion_chars } => {
                // 服务端未返回 usage，基于字符数估算
                // 估算规则：中文 ~2 chars/token，英文/代码 ~4 chars/token
                // 简单混合估算：~3 chars/token
                self.completion_tokens = (completion_chars / 3).max(1);
                self.has_real_usage = false;
            }
        }
    }

    /// 获取累积的文本
    pub fn content(&self) -> &str {
        &self.content
    }

    /// 检测累积文本中是否已包含完整的 JSON 对象
    ///
    /// 使用大括号计数，从第一个 `{` 到其闭合 `}` 为止
    pub fn is_json_complete(&self) -> bool {
        find_first_json_end(&self.content).is_some()
    }

    /// 消费累积器，返回最终文本
    pub fn into_content(self) -> String {
        self.content
    }

    /// 获取 token 用量 (prompt_tokens, completion_tokens, has_real_usage)
    pub fn token_stats(&self) -> (u64, u64, bool) {
        (
            self.prompt_tokens,
            self.completion_tokens,
            self.has_real_usage,
        )
    }

    /// 追加一个流式 tool_call delta
    pub fn push_tool_call_delta(&mut self, delta: &super::tool_types::StreamToolCallDelta) {
        self.has_tool_calls = true;
        self.tool_call_acc.push(delta);
    }

    /// 是否收到过 tool_calls
    pub fn has_tool_calls(&self) -> bool {
        self.has_tool_calls
    }

    /// 消费累积器，返回完整的 tool_calls 列表
    pub fn into_tool_calls(self) -> Vec<super::tool_types::ToolCall> {
        self.tool_call_acc.into_tool_calls()
    }

    /// 同时消费 content、tool_calls 和 reasoning_content
    pub fn into_parts(self) -> (String, Vec<super::tool_types::ToolCall>, String) {
        let tool_calls = self.tool_call_acc.into_tool_calls();
        (self.content, tool_calls, self.reasoning_content)
    }

    /// 消费 content 和 tool_calls（不含 reasoning）
    pub fn into_content_and_tool_calls(self) -> (String, Vec<super::tool_types::ToolCall>) {
        let tool_calls = self.tool_call_acc.into_tool_calls();
        (self.content, tool_calls)
    }
}

// ============================================================================
// SSE 解析
// ============================================================================

/// 将 reqwest Response 的 bytes_stream 解析为 LlmStream
pub fn parse_sse_stream(response: reqwest::Response) -> LlmStream {
    let stream = async_stream::stream! {
        let mut raw_buffer: Vec<u8> = Vec::with_capacity(4096);
        let mut stream = response.bytes_stream();
        let mut last_usage: Option<(u64, u64)> = None;
        let mut completion_content = String::new();
        let mut chunk_count: u64 = 0;
        let mut raw_chunks = String::with_capacity(2048);

        while let Some(chunk_result) = futures_util::StreamExt::next(&mut stream).await {
            let bytes = match chunk_result {
                Ok(b) => b,
                Err(e) => {
                    yield Err(anyhow::anyhow!("SSE stream error: {}", e));
                    return;
                }
            };

            raw_buffer.extend_from_slice(&bytes);

            // 按字节序列 b"\n\n" 拆分 SSE 事件
            let sep = b"\n\n";
            while let Some(pos) = raw_buffer.windows(sep.len()).position(|w| w == sep) {
                let event_bytes: Vec<u8> = raw_buffer.drain(..pos + sep.len()).collect();
                // 丢弃尾部 \n\n，只保留事件内容
                let event_text = String::from_utf8_lossy(&event_bytes[..event_bytes.len().saturating_sub(sep.len())]);

                // 处理事件内的每一行
                for line in event_text.lines() {
                    let line = line.trim();
                    if !line.starts_with("data: ") {
                        continue;
                    }
                    let data = &line[6..];

                    if data == "[DONE]" {
                        // 空响应诊断：有 chunks 但无 content，输出原始 SSE 数据
                        if completion_content.trim().is_empty() && chunk_count > 0 {
                            tracing::warn!(
                                "[SSE 空响应诊断] {} chunks received but content is empty. Raw chunks:\n{}",
                                chunk_count,
                                &raw_chunks[..raw_chunks.len().min(1500)]
                            );
                        }
                        // [DONE] 不携带 usage，使用最后记录的 usage
                        if let Some((pt, ct)) = last_usage {
                            yield Ok(StreamChunk::Done {
                                prompt_tokens: pt,
                                completion_tokens: ct,
                            });
                        } else {
                            // 服务端从未返回 usage，基于已累积的内容估算
                            yield Ok(StreamChunk::DoneEstimation {
                                completion_chars: completion_content.len() as u64,
                            });
                        }
                        return;
                    }

                    match serde_json::from_str::<OpenAIStreamResponse>(data) {
                        Ok(resp) => {
                            chunk_count += 1;
                            // 累积原始 chunk 用于空响应诊断
                            if chunk_count <= 8 {
                                raw_chunks.push_str(data);
                                raw_chunks.push('\n');
                            }
                            let mut has_content = false;
                            let has_tool_calls = resp.choices.iter().any(|c| {
                                c.delta.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty())
                            });
                            for choice in &resp.choices {
                                if let Some(ref content) = choice.delta.content
                                    && !content.is_empty()
                                {
                                    has_content = true;
                                    completion_content.push_str(content);
                                    yield Ok(StreamChunk::Delta(content.clone()));
                                }
                                if let Some(ref reasoning) = choice.delta.reasoning_content
                                    && !reasoning.is_empty()
                                {
                                    has_content = true;
                                    yield Ok(StreamChunk::ReasoningDelta(reasoning.clone()));
                                }
                                if let Some(ref tool_calls) = choice.delta.tool_calls {
                                    for tc in tool_calls {
                                        yield Ok(StreamChunk::ToolCallDelta(tc.clone()));
                                    }
                                }
                            }
                            if !has_content && !has_tool_calls {
                                tracing::debug!(
                                    "SSE chunk #{}: no delta content (total_content={} chars)",
                                    chunk_count,
                                    completion_content.len(),
                                );
                            }
                            // 记录 usage（即使不是最后一个 chunk）
                            if let Some(ref usage) = resp.usage {
                                last_usage = Some((usage.prompt_tokens, usage.completion_tokens));
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[SSE 解析失败] chunk #{}: {} (data: {})",
                                chunk_count + 1,
                                e,
                                &data[..data.len().min(200)]
                            );
                        }
                    }
                }
            }
        }
    };

    Box::pin(stream)
}

// ============================================================================
// JSON 闭合检测（复用 client.rs 逻辑）
// ============================================================================

/// 从第一个 `{` 开始，用大括号计数找完整 JSON 对象的结束位置
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
                return Some(i + 1);
            }
        }
    }
    None
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accumulator_basic() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamChunk::Delta("hello".to_string()));
        acc.push(StreamChunk::Delta(" world".to_string()));
        acc.push(StreamChunk::Done {
            prompt_tokens: 10,
            completion_tokens: 5,
        });
        assert_eq!(acc.content(), "hello world");
        let (pt, ct, has_real) = acc.token_stats();
        assert_eq!(pt, 10);
        assert_eq!(ct, 5);
        assert!(has_real);
    }

    #[test]
    fn test_accumulator_estimation() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamChunk::Delta("hello world".to_string()));
        acc.push(StreamChunk::DoneEstimation {
            completion_chars: 11,
        });
        assert_eq!(acc.content(), "hello world");
        let (pt, ct, has_real) = acc.token_stats();
        assert_eq!(pt, 0); // prompt_tokens 未记录
        assert_eq!(ct, 11 / 3); // ~3 chars/token
        assert!(!has_real);
    }

    #[test]
    fn test_json_complete_detection() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamChunk::Delta(r#"{"name":"test""#.to_string()));
        assert!(!acc.is_json_complete());
        acc.push(StreamChunk::Delta(r#","value":42}"#.to_string()));
        assert!(acc.is_json_complete());
    }

    #[test]
    fn test_json_complete_with_trailing_text() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamChunk::Delta(
            r#"{"action":"rest"}This is extra text"#.to_string(),
        ));
        assert!(acc.is_json_complete());
    }

    #[test]
    fn test_json_not_complete_unclosed_bracket() {
        let mut acc = StreamAccumulator::new();
        acc.push(StreamChunk::Delta(
            r#"{"name":"test","items":[1,2,3"#.to_string(),
        ));
        assert!(!acc.is_json_complete());
    }

    #[test]
    fn test_find_first_json_end() {
        assert_eq!(find_first_json_end(r#"{"a":1} extra"#), Some(7));
        assert_eq!(find_first_json_end(r#"{"a":{"b":2}}"#), Some(13));
        assert_eq!(find_first_json_end("no json"), None);
    }
}
