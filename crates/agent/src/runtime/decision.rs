// ============================================================================
// Cognitive Decision - 认知引擎决策
// ============================================================================
//
// 人魂直连 WorldState，单次 LLM 调用输出结构化 Intent。
// CognitiveValidator 在内部重试循环中执行质量审查。
// 天魂翻译步骤已消除。

use crate::component::llm::{ErrorAction, classify_llm_error};
use crate::soul::actor::{CognitiveChain, CognitiveEngine};
use crate::soul::reflector::cognitive_validator::CognitiveValidator;
use cyber_jianghu_protocol::{Intent, WorldState};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use tracing::{error, warn};

/// Cognitive 决策配置
pub struct CognitiveDecisionConfig {
    /// 最大重试次数
    pub max_retries: usize,
}

impl Default for CognitiveDecisionConfig {
    fn default() -> Self {
        Self { max_retries: 12 }
    }
}

/// 创建认知决策函数
///
/// 使用认知引擎进行决策（旧式回调，不接收 WorldState）
pub fn cognitive_decision(
    engine: Arc<CognitiveEngine>,
    _config: CognitiveDecisionConfig,
) -> impl Fn(i64, uuid::Uuid) -> BoxFuture<'static, Intent> + Send + Sync + 'static {
    move |tick_id: i64, agent_id: uuid::Uuid| {
        let engine = engine.clone();

        Box::pin(async move {
            // 运行认知流程
            match engine.think(tick_id, agent_id).await {
                Ok(chain) => chain.final_intent,
                Err(e) => {
                    error!("[cognitive] Decision failed: {}", e);
                    Intent::new(agent_id, tick_id, "休整", None)
                        .with_thought(format!("认知失败: {}", e))
                }
            }
        })
    }
}

/// 创建带 CognitiveChain 返回的认知决策函数（人魂直连 WorldState）
///
/// 人魂直接接收 WorldState，输出结构化 Intent（action_type + action_data 精确 ID）。
/// CognitiveChain 供 soul_cycle_recorder 记录用。
#[allow(clippy::type_complexity)]
pub fn cognitive_decision_with_chain(
    engine: Arc<CognitiveEngine>,
    max_retries: usize,
) -> impl Fn(&WorldState, &str, Option<&str>, i32) -> BoxFuture<'static, (Intent, Option<CognitiveChain>)>
+ Send
+ Sync
+ 'static {
    move |world_state: &WorldState, memory_context: &str, feedback: Option<&str>, soul_cycle_attempt: i32| {
        let engine = engine.clone();
        let world_state = world_state.clone();
        let memory_context = memory_context.to_string();
        let mut feedback = feedback.map(|s| s.to_string());

        Box::pin(async move {
            let mut last_error = String::new();
            let mut last_chain: Option<CognitiveChain> = None;
            let mut failed_attempts: usize = 0;

            for attempt in 0..=max_retries {
                let _ = attempt; // 内层认知校验重试序号（不影响 trace 的 soul_cycle_attempt）
                match engine
                    .think_direct(&world_state, &memory_context, feedback.as_deref(), soul_cycle_attempt)
                    .await
                {
                    Ok(chain) => {
                        let final_intent = chain.final_intent.clone();
                        last_chain = Some(chain.clone());

                        // 推送到对话历史（长窗口）
                        // user 字段使用 world_state 摘要而非 memory_context，
                        // 避免工作记忆（环境观察/紧急事件）伪装成对话历史
                        let ws_summary = format!(
                            "Tick {} @ {}",
                            world_state.tick_id, &world_state.location.node_id,
                        );
                        // assistant 字段携带实际内容，让 LLM 在对话历史中看到自己说过什么
                        // 截断防止 token 膨胀（200 中文字 ≈ 300 tokens，8 轮 ≈ 2400 tokens ≈ 7.5% 窗口）
                        const ASSISTANT_SUMMARY_CHAR_LIMIT: usize = 200;
                        let assistant_summary = match final_intent
                            .action_data
                            .as_ref()
                            .and_then(|d| d.get("content"))
                            .and_then(|v| v.as_str())
                        {
                            Some(content) if !content.is_empty() => format!(
                                "{}: {}",
                                final_intent.action_type,
                                content
                                    .chars()
                                    .filter(|c| !c.is_control())
                                    .take(ASSISTANT_SUMMARY_CHAR_LIMIT)
                                    .collect::<String>()
                            ),
                            _ => final_intent.action_type.to_string(),
                        };
                        engine.push_conversation_turn(
                            world_state.tick_id,
                            ws_summary,
                            assistant_summary,
                            engine.take_last_reasoning_content(),
                        );

                        // CognitiveValidator: 验证认知链质量
                        let validator = CognitiveValidator::new(chain.persona.clone());
                        let validation = validator.validate(&chain);
                        if validation.is_valid {
                            return (final_intent, Some(chain));
                        }

                        let reason = validation.reason.unwrap_or_default();
                        let suggestion = validation.suggestion.unwrap_or_default();
                        warn!(
                            "[cognitive] Validator rejected (attempt {}/{}): {} | suggestion: {}",
                            attempt + 1,
                            max_retries + 1,
                            reason,
                            suggestion
                        );

                        if attempt == max_retries {
                            warn!(
                                "[cognitive] Max retries reached, using intent despite validation failure"
                            );
                            return (final_intent, Some(chain));
                        }
                    }
                    Err(e) => {
                        failed_attempts += 1;
                        last_error = e.to_string();
                        error!("[cognitive] Attempt {} failed: {}", attempt + 1, e);

                        // 将解析错误注入重试 feedback，让 LLM 知道上次哪里错了
                        feedback = Some(format!(
                            "系统提示：你上一次输出格式有误（{}），请确保严格输出合法的JSON对象，不要在JSON外添加任何文本。",
                            last_error
                        ));

                        // 按统一分类决定是否中止重试
                        // call_with_fallback 已尝试所有可用客户端，继续重试无意义
                        let (action, _reason) = classify_llm_error(&e);
                        match action {
                            ErrorAction::Retry => {
                                // 网络瞬时故障，可能恢复，继续重试
                            }
                            other => {
                                warn!(
                                    "[cognitive] Aborting retries (action={:?}): {}",
                                    other, last_error
                                );
                                break;
                            }
                        }
                    }
                }
            }

            let idle_intent = Intent::new(
                world_state.agent_id.unwrap_or_default(),
                world_state.tick_id,
                "休整",
                None,
            )
            .with_thought(format!(
                "认知失败({}/{}次重试): {}",
                failed_attempts, max_retries, last_error
            ));
            (idle_intent, last_chain)
        })
    }
}
