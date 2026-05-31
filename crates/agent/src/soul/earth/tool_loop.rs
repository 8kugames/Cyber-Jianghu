// ============================================================================
// 地魂 Tool Loop — 共享 tool calling 循环逻辑
// ============================================================================
//
// 从 DirectLlmClient 抽取的 agent 级工具循环。
// Cognitive/Claw 模式区别仅 LLM 在内部还是外部，
// 循环逻辑（LoopGuard / Budget / tool 执行）属于 agent 本身。
//
// LLM 接入点通过 send_chat_exchange trait 方法抽象，
// DirectLlmClient 用 HTTP，OpenClawBridge 用 WebSocket。

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::component::llm::tool_types::{ToolDefinition, ToolExecutor};
use crate::component::llm::{ChatExchangeConfig, ChatMessage, LlmClient};

use super::budget::ToolResultBudget;
/// tool loop 的返回结果
pub(crate) struct ToolLoopResult {
    pub content: String,
    pub reasoning_content: Option<String>,
}

use super::config::EarthSoulConfig;
use super::loop_guard::{LoopGuard, LoopGuardAction};

/// 共享 tool calling 循环
///
/// 接收预构建的消息列表，执行多轮 tool-calling 直到 LLM 返回文本或超时。
/// 集成 ToolResultBudget（F1）、LoopGuard（F2）、Error Signaling（F3）。
pub(crate) async fn run_tool_loop(
    llm: &dyn LlmClient,
    messages: Vec<ChatMessage>,
    tools: &[ToolDefinition],
    executor: &dyn ToolExecutor,
    max_rounds: usize,
    earth_config: Option<&EarthSoulConfig>,
    llm_config: ChatExchangeConfig,
) -> Result<ToolLoopResult> {
    let mut messages = messages;

    let mut budget = earth_config
        .filter(|c| c.tool_budget.enabled)
        .map(|c| ToolResultBudget::new(&c.tool_budget, llm.context_window_tokens()));
    let mut guard = earth_config
        .filter(|c| c.loop_guard.enabled)
        .map(|c| LoopGuard::new(&c.loop_guard));

    for round in 0..max_rounds {
        // Pre-check: budget exhausted
        if let Some(ref b) = budget
            && b.is_exhausted()
        {
            warn!(
                "[地魂] Tool result 预算耗尽 ({} chars), 提前退出",
                b.used_chars()
            );
            return forced_text_exit(llm, messages, llm_config.clone()).await;
        }

        if round == 0 {
            let tool_names: Vec<&str> = tools.iter().map(|t| t.function.name.as_str()).collect();
            info!(
                "[地魂] Tool loop 开始, tools={:?}, max_rounds={}",
                tool_names, max_rounds
            );
        }

        let response = llm
            .send_chat_exchange(messages.clone(), Some(tools), llm_config.clone())
            .await?;

        debug!(
            "[地魂] API 响应: tool_calls={}, content_len={}, content_preview={}",
            response
                .tool_calls
                .as_ref()
                .map(|tc| format!(
                    "{:?}",
                    tc.iter()
                        .map(|t| t.function.name.clone())
                        .collect::<Vec<_>>()
                ))
                .unwrap_or_else(|| "None".to_string()),
            response.content.as_ref().map(|c| c.len()).unwrap_or(0),
            response
                .content
                .as_ref()
                .map(|c| c.chars().take(1000).collect::<String>())
                .unwrap_or_default(),
        );

        let has_tool_calls = response
            .tool_calls
            .as_ref()
            .map(|tc| !tc.is_empty())
            .unwrap_or(false);

        if !has_tool_calls {
            let content = response.content.unwrap_or_default();
            info!(
                "[地魂] LLM 未调用任何 tool，直接返回文本 ({} chars), preview: {}",
                content.len(),
                content.chars().take(2000).collect::<String>()
            );

            // 提取 JSON：纯 JSON 直接通过，否则从内容中提取含 actions 标记的决策 JSON
            let trimmed = content.trim();
            if trimmed.starts_with('{') {
                return Ok(ToolLoopResult {
                    content,
                    reasoning_content: response.reasoning_content,
                });
            }

            if let Some(json) = extract_json_object(&content) {
                info!(
                    "[地魂] 从 LLM 输出中提取到 JSON ({} chars), 跳过 forced_text_exit",
                    json.len()
                );
                return Ok(ToolLoopResult {
                    content: json,
                    reasoning_content: response.reasoning_content,
                });
            }

            warn!(
                "[地魂] LLM 返回无 JSON 内容 ({} chars), 转为强制JSON退出, preview: {}",
                content.len(),
                content.chars().take(200).collect::<String>()
            );
            messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: Some(content),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: response.reasoning_content,
            });
            return forced_text_exit(llm, messages, llm_config).await;
        }

        let tool_calls = response
            .tool_calls
            .as_ref()
            .expect("tool_calls must exist when finish_reason is tool_calls");
        let call_names: Vec<&str> = tool_calls
            .iter()
            .map(|tc| tc.function.name.as_str())
            .collect();
        info!(
            "[地魂] LLM 请求调用 {} 个 tool: {:?}",
            tool_calls.len(),
            call_names
        );

        // Push assistant message with tool_calls
        messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: response.content,
            tool_calls: Some(tool_calls.clone()),
            tool_call_id: None,
            name: None,
            reasoning_content: response.reasoning_content.clone(),
        });

        for (i, tc) in tool_calls.iter().enumerate() {
            // Loop guard check (F2) — 渐进策略：Warn → Terminate
            if let Some(ref mut g) = guard {
                match g.check(&tc.function.name) {
                    LoopGuardAction::Terminate => {
                        warn!(
                            "[地魂] Loop guard 截断: 连续调用 '{}' 超限",
                            tc.function.name
                        );
                        // 为当前及所有剩余 tool_calls 推送占位 result
                        for remaining in &tool_calls[i..] {
                            messages.push(ChatMessage::tool_result(
                                &remaining.id,
                                &remaining.function.name,
                                "[已获知足够信息，直接回答]",
                            ));
                        }
                        return forced_text_exit(llm, messages, llm_config.clone()).await;
                    }
                    LoopGuardAction::Warn(_) => {
                        warn!("[地魂] Loop guard 警告: 连续调用 '{}'", tc.function.name);
                    }
                    LoopGuardAction::Proceed => {}
                }
            }

            // Execute + error signaling (F3)
            let args = tc.parse_arguments().unwrap_or(serde_json::json!({}));
            info!("[地魂] 执行 tool: {}({})", tc.function.name, args);

            let raw_result = match executor.execute(&tc.function.name, &args).await {
                Ok(val) => val,
                Err(e) => {
                    warn!("[地魂] Tool '{}' 执行失败: {}", tc.function.name, e);
                    let error_str =
                        format!("[工具调用失败] 工具: {} | 原因: {}", tc.function.name, e);
                    messages.push(ChatMessage::tool_result(
                        &tc.id,
                        &tc.function.name,
                        &error_str,
                    ));
                    continue;
                }
            };

            // Budget 处理 (F1) — JSON 感知：紧凑化 → 字符截断兜底
            let processed = match &mut budget {
                Some(b) => {
                    if b.is_exhausted() {
                        ToolResultBudget::exhausted_message().to_string()
                    } else {
                        b.process(&tc.function.name, &raw_result)
                    }
                }
                None => raw_result.to_string(),
            };

            // LoopGuard Warn 后置拼接（budget 处理后再 prepend，不破坏 JSON）
            let final_result = if let Some(ref mut g) = guard
                && let Some(warning) = g.take_pending_warning()
            {
                format!("[系统提示] {}\n{}", warning, processed)
            } else {
                processed
            };

            info!(
                "[地魂] Tool {} 结果: {}",
                tc.function.name,
                final_result.chars().take(2000).collect::<String>()
            );

            messages.push(ChatMessage::tool_result(
                &tc.id,
                &tc.function.name,
                &final_result,
            ));
        }
    }

    // Max rounds exhausted → forced text exit
    warn!(
        "[地魂] Tool loop 耗尽 max_rounds ({}), 执行强制文本退出",
        max_rounds
    );
    forced_text_exit(llm, messages, llm_config).await
}

/// 强制文本退出：不发送 tool 定义，迫使模型基于已积累上下文输出文本决策
async fn forced_text_exit(
    llm: &dyn LlmClient,
    mut messages: Vec<ChatMessage>,
    llm_config: ChatExchangeConfig,
) -> Result<ToolLoopResult> {
    warn!("[地魂] 执行强制文本退出");

    // 追加引导消息：显式要求 JSON 格式输出
    messages.push(ChatMessage::user(
        "你已充分了解周围情况。现在请严格按照系统提示中的JSON格式输出你的决策。只输出JSON对象，不要输出任何其他文本。",
    ));

    let response = llm.send_chat_exchange(messages, None, llm_config).await?;

    let content = response.content.unwrap_or_default();
    if content.trim().is_empty() {
        warn!("[地魂] 强制文本退出返回空内容，agent 本轮可能无决策");
    }

    Ok(ToolLoopResult {
        content,
        reasoning_content: response.reasoning_content,
    })
}

/// 决策 JSON 的确定性标识字段
const DECISION_MARKER: &str = "actions";

/// 从 LLM 输出中提取决策 JSON 对象。
///
/// 策略：遍历所有 JSON object，优先找含 `DECISION_MARKER` 的（确定性决策），
/// 找不到则 fallback 最后一个 object（兼容旧格式）。
/// 用 `StreamDeserializer` 容忍前后文本。
fn extract_json_object(content: &str) -> Option<String> {
    let start = content.find('{')?;
    let json_candidate = &content[start..];
    let stream = serde_json::Deserializer::from_str(json_candidate)
        .into_iter::<serde_json::Value>();

    let mut last_object: Option<serde_json::Value> = None;
    let mut marked_object: Option<serde_json::Value> = None;

    for value in stream.flatten() {
        if value.is_object() {
            if value.get(DECISION_MARKER).is_some() {
                marked_object = Some(value);
            } else {
                last_object = Some(value);
            }
        }
    }

    marked_object
        .or(last_object)
        .map(|v| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_object_pure_json() {
        let json = r#"{"action_type":"喝水","action_data":{"item_id":"水"}}"#;
        let extracted = extract_json_object(json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extracted).unwrap();
        assert_eq!(parsed["action_type"], "喝水");
        assert_eq!(parsed["action_data"]["item_id"], "水");
    }

    #[test]
    fn test_extract_json_object_reasoning_then_json() {
        let content = "Good, I have:\n- 水: 7\n\n{\"action_type\":\"喝水\",\"item\":\"水\"}";
        let extracted = extract_json_object(content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extracted).unwrap();
        assert_eq!(parsed["action_type"], "喝水");
    }

    #[test]
    fn test_extract_json_object_no_json() {
        assert_eq!(extract_json_object("just plain text, no json here"), None);
    }

    #[test]
    fn test_extract_json_object_json_array_returns_none() {
        // JSON array 不是 object，不应提取
        assert_eq!(extract_json_object("[1,2,3]"), None);
    }

    #[test]
    fn test_extract_json_object_nested_braces() {
        let content = "thinking...\n{\"a\":{\"b\":1},\"c\":2}";
        let extracted = extract_json_object(content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extracted).unwrap();
        assert_eq!(parsed["a"]["b"], 1);
        assert_eq!(parsed["c"], 2);
    }

    #[test]
    fn test_extract_json_object_trailing_text() {
        let content = "reasoning...\n{\"action_type\":\"喝水\"}\n\ndone.";
        let extracted = extract_json_object(content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extracted).unwrap();
        assert_eq!(parsed["action_type"], "喝水");
    }

    #[test]
    fn test_extract_json_object_surrounded_text() {
        let content = "Before.\n{\"x\":1}\nAfter.";
        let extracted = extract_json_object(content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extracted).unwrap();
        assert_eq!(parsed["x"], 1);
    }

    #[test]
    fn test_extract_json_object_multiple_prefers_marked() {
        // 第一个无 actions，第二个有 actions → 优先取有 actions 的
        let content = r#"analysis...
{"step":"reasoning","thirst":99}
{"actions":[{"action_type":"喝水","action_data":{"item_id":"水"}}]}"#;
        let extracted = extract_json_object(content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extracted).unwrap();
        assert!(parsed.get("actions").is_some(), "should pick JSON with actions");
        assert!(parsed.get("step").is_none(), "should not be the unmarked JSON");
    }

    #[test]
    fn test_extract_json_object_marked_before_unmarked() {
        // 有 actions 的在前面，无 actions 的在后面 → 仍取有 actions 的
        let content = r#"thinking...
{"actions":[{"action_type":"喝水"}]}
{"note":"some afterthought"}"#;
        let extracted = extract_json_object(content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extracted).unwrap();
        assert!(parsed.get("actions").is_some());
    }

    #[test]
    fn test_extract_json_object_no_marker_fallback_last() {
        // 都没有 actions → fallback 最后一个
        let content = r#"{"a":1}
{"b":2}"#;
        let extracted = extract_json_object(content).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&extracted).unwrap();
        assert_eq!(parsed["b"], 2);
    }
}
