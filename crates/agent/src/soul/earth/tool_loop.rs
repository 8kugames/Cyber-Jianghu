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
) -> Result<String> {
    let mut messages = messages;

    let mut budget = earth_config
        .filter(|c| c.tool_budget.enabled)
        .map(|c| ToolResultBudget::new(&c.tool_budget));
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
                .map(|c| c.chars().take(100).collect::<String>())
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
                content.chars().take(200).collect::<String>()
            );
            return Ok(content);
        }

        let tool_calls = response.tool_calls.as_ref().expect("tool_calls must exist when finish_reason is tool_calls");
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
                                "[系统截断] 连续调用超限，本次调用已取消",
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

            let mut raw_result = match executor.execute(&tc.function.name, &args).await {
                Ok(val) => val.to_string(),
                Err(e) => {
                    warn!("[地魂] Tool '{}' 执行失败: {}", tc.function.name, e);
                    format!("[工具调用失败] 工具: {} | 原因: {}", tc.function.name, e)
                }
            };

            // 将 LoopGuard Warn 作为 tool result 前缀注入
            if let Some(ref mut g) = guard
                && let Some(warning) = g.take_pending_warning()
            {
                raw_result = format!("[系统提示] {}\n{}", warning, raw_result);
            }

            // Budget truncation (F1)
            let truncated = match &mut budget {
                Some(b) => {
                    if b.is_exhausted() {
                        ToolResultBudget::exhausted_message().to_string()
                    } else {
                        b.truncate(&tc.function.name, &raw_result)
                    }
                }
                None => raw_result,
            };

            info!(
                "[地魂] Tool {} 结果: {}",
                tc.function.name,
                truncated.chars().take(200).collect::<String>()
            );

            messages.push(ChatMessage::tool_result(
                &tc.id,
                &tc.function.name,
                &truncated,
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
) -> Result<String> {
    warn!("[地魂] 执行强制文本退出");

    // 追加引导消息，防止模型在 tools 被移除后返回空响应
    messages.push(ChatMessage::user(
        "工具调用已结束。请直接输出你的决策 JSON，不要再调用任何工具。",
    ));

    let response = llm.send_chat_exchange(messages, None, llm_config).await?;

    let content = response.content.unwrap_or_default();
    if content.is_empty() {
        warn!("[地魂] 强制文本退出返回空内容，agent 本轮可能无决策");
    }

    Ok(content)
}
