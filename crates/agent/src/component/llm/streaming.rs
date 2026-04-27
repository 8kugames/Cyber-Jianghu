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

use super::openai_types::OpenAIStreamResponse;

// ============================================================================
// 公共类型
// ============================================================================

/// SSE 流的一个 chunk
#[derive(Debug, Clone)]
pub enum StreamChunk {
    /// 文本增量
    Delta(String),
    /// 流结束（含 token 用量）
    Done {
        prompt_tokens: u64,
        completion_tokens: u64,
    },
}

/// SSE 流类型
pub type LlmStream = Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>;

// ============================================================================
// 流累积器
// ============================================================================

/// 将 StreamChunk 累积为完整响应文本
pub struct StreamAccumulator {
    content: String,
    prompt_tokens: u64,
    completion_tokens: u64,
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
            prompt_tokens: 0,
            completion_tokens: 0,
        }
    }

    /// 追加一个 chunk
    pub fn push(&mut self, chunk: StreamChunk) {
        match chunk {
            StreamChunk::Delta(text) => self.content.push_str(&text),
            StreamChunk::Done {
                prompt_tokens,
                completion_tokens,
            } => {
                self.prompt_tokens = prompt_tokens;
                self.completion_tokens = completion_tokens;
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

    /// 获取 token 用量 (prompt_tokens, completion_tokens)
    pub fn token_stats(&self) -> (u64, u64) {
        (self.prompt_tokens, self.completion_tokens)
    }
}

// ============================================================================
// SSE 解析
// ============================================================================

/// 将 reqwest Response 的 bytes_stream 解析为 LlmStream
pub fn parse_sse_stream(response: reqwest::Response) -> LlmStream {
    let stream = async_stream::stream! {
        let mut buffer = String::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk_result) = futures_util::StreamExt::next(&mut stream).await {
            let bytes = match chunk_result {
                Ok(b) => b,
                Err(e) => {
                    yield Err(anyhow::anyhow!("SSE stream error: {}", e));
                    return;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&bytes));

            // 按换行拆分 SSE 事件
            while let Some(pos) = buffer.find("\n\n") {
                let event_text = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                // 处理事件内的每一行
                for line in event_text.lines() {
                    let line = line.trim();
                    if !line.starts_with("data: ") {
                        continue;
                    }
                    let data = &line[6..];

                    if data == "[DONE]" {
                        yield Ok(StreamChunk::Done {
                            prompt_tokens: 0,
                            completion_tokens: 0,
                        });
                        return;
                    }

                    match serde_json::from_str::<OpenAIStreamResponse>(data) {
                        Ok(resp) => {
                            for choice in &resp.choices {
                                if let Some(ref content) = choice.delta.content
                                    && !content.is_empty()
                                {
                                    yield Ok(StreamChunk::Delta(content.clone()));
                                }
                            }
                            // 最后一个 chunk 可能包含 usage
                            if let Some(ref usage) = resp.usage
                                && (resp.choices.is_empty()
                                    || resp
                                        .choices
                                        .last()
                                        .map(|c| c.finish_reason.is_some())
                                        .unwrap_or(false))
                            {
                                yield Ok(StreamChunk::Done {
                                    prompt_tokens: usage.prompt_tokens,
                                    completion_tokens: usage.completion_tokens,
                                });
                            }
                        }
                        Err(e) => {
                            // 跳过无法解析的 chunk（可能是空数据）
                            tracing::debug!("SSE parse skip: {} (data: {})", e, &data[..data.len().min(100)]);
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
