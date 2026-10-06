// ============================================================================
// Cognitive Decision - 认知引擎决策
// ============================================================================
//
// 人魂直连 WorldState，单次 LLM 调用输出结构化 Intent。
// CognitiveValidator 在内部重试循环中执行质量审查。
// 天魂翻译步骤已消除。
//
// 决策模型两段式管线（decision_model.enabled 时优先尝试，失败/低置信整体
// 回退本文件的既有 LLM 路径）：
//   1. 人魂认知-only 调用（think_cognition_only，不写 actions）
//   2. 决策模型逐问题读出（act1 → act2 → 实体绑定，一次一问）
//   3. 置信度门控（act1 ≥ threshold 且可绑定）→ 构造 Intents
//   4. 天魂四层照常审查（本模块零改动感知）

use crate::component::decision_model::{self as dm, DecisionModelManager};
use crate::component::llm::{ErrorAction, classify_llm_error};
use crate::soul::actor::engine::cognition::CognitionOutput;
use crate::soul::actor::{CognitiveChain, CognitiveEngine, CognitiveStage, StageOutput};
use crate::soul::reflector::cognitive_validator::CognitiveValidator;
use cyber_jianghu_protocol::{Intent, WorldState};
use futures_util::future::BoxFuture;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// 判定错误文本是否为 JSON 格式解析错误（serde 错误家族特征）。
fn is_format_parse_error(msg: &str) -> bool {
    msg.contains("invalid type")
        || msg.contains("expected value")
        || msg.contains("trailing characters")
        || msg.contains("missing field")
        || msg.contains("EOF while parsing")
        || msg.contains("key must be")
        || msg.contains("duplicate field")
}

/// 解析失败重试反馈构造。
///
/// 格式解析错误不注入 serde 技术细节原文：`invalid type: null ... column 232`
/// 这类细节会把输出敏感的模型推离分布（2026-09-28 灰度实证：小模型在该反馈
/// 形态下模仿错误内容、连环崩坏）；保留「格式有误」信号本身即可引导自纠。
/// 连续第 2 次格式失败起清空反馈干净重试，阻断 in-context 污染累积。
/// 非格式错误（网络等）维持原行为：携带错误原文注入反馈。
pub(crate) fn retry_feedback_for_error(
    err_text: &str,
    format_fail_streak: usize,
) -> Option<String> {
    if is_format_parse_error(err_text) {
        if format_fail_streak >= 2 {
            return None;
        }
        return Some(
            "系统提示：你上一次输出格式有误，请确保严格输出合法的JSON对象，不要在JSON外添加任何文本。"
                .to_string(),
        );
    }
    Some(format!(
        "系统提示：你上一次输出格式有误（{}），请确保严格输出合法的JSON对象，不要在JSON外添加任何文本。",
        err_text
    ))
}

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
///
/// `decision_model_slot` 为共享槽位（`RwLock<Option<Arc<Manager>>>`，与 HTTP API 状态
/// 同源）：每 tick 读取一次，面板换装/开关新 manager 后下一 tick 即热生效；槽位为空
/// 或 enabled=false 时直接走既有 LLM 路径（完整 JSON + 重试机制），既有路径行为一字不变。
#[allow(clippy::type_complexity)]
pub fn cognitive_decision_with_chain(
    engine: Arc<CognitiveEngine>,
    max_retries: usize,
    decision_model_slot: Arc<RwLock<Option<Arc<DecisionModelManager>>>>,
) -> impl Fn(
    &WorldState,
    &str,
    Option<&str>,
    i32,
) -> BoxFuture<'static, (Intent, Option<CognitiveChain>)>
+ Send
+ Sync
+ 'static {
    move |world_state: &WorldState,
          memory_context: &str,
          feedback: Option<&str>,
          soul_cycle_attempt: i32| {
        let engine = engine.clone();
        let world_state = world_state.clone();
        let memory_context = memory_context.to_string();
        let mut feedback = feedback.map(|s| s.to_string());
        let decision_model_slot = decision_model_slot.clone();

        Box::pin(async move {
            // ── 决策模型两段式路径（每 tick 从共享槽位读取；面板热换装下一 tick 生效）──
            let decision_model = decision_model_slot.read().await.clone();
            if let Some(ref model) = decision_model
                && model.is_enabled()
            {
                dm::metrics::record_tick_attempt();
                match decide_via_model(
                    &engine,
                    model,
                    &world_state,
                    &memory_context,
                    feedback.as_deref(),
                    soul_cycle_attempt,
                )
                .await
                {
                    Ok(Some((intent, chain))) => {
                        dm::metrics::record_taken();
                        return (intent, Some(chain));
                    }
                    Ok(None) => {
                        info!(
                            "[decision_model] tick {} 门控未通过，回退 LLM 决策路径",
                            world_state.tick_id
                        );
                    }
                    Err(e) => {
                        dm::metrics::record_fallback_error();
                        warn!(
                            "[decision_model] tick {} 决策路径失败（回退 LLM 决策路径）: {:#}",
                            world_state.tick_id, e
                        );
                    }
                }
            }

            let mut last_error = String::new();
            let mut last_chain: Option<CognitiveChain> = None;
            let mut failed_attempts: usize = 0;
            let mut format_fail_streak: usize = 0;

            // 墙钟预算：flaky LLM 下 max_retries 次重试 × 120s 请求超时可空转数十分钟
            // （2026-09-15 柳青崖事故）。超预算即跳出循环走休整降级，
            // chaos 恢复机制在后续 tick 兜底。
            const RETRY_BUDGET_SECS: u64 = 300;
            let budget_started = std::time::Instant::now();
            for attempt in 0..=max_retries {
                if budget_started.elapsed() > std::time::Duration::from_secs(RETRY_BUDGET_SECS) {
                    warn!(
                        "[cognitive] 重试预算耗尽（{}s），中止本轮决策走休整降级",
                        RETRY_BUDGET_SECS
                    );
                    break;
                }
                let _ = attempt; // 内层认知校验重试序号（不影响 trace 的 soul_cycle_attempt）
                match engine
                    .think_direct(
                        &world_state,
                        &memory_context,
                        feedback.as_deref(),
                        soul_cycle_attempt,
                    )
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
                            world_state.tick_id, world_state.location.node_id,
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
                        // CognitiveValidator: 验证认知链质量
                        // （先验证后写历史：被驳回的轮次不入对话史，
                        //   否则脏轮次会永久驻留并在后续 prompt 中回放）
                        let validator = CognitiveValidator::new(chain.persona.clone());
                        let validation = validator.validate(&chain);
                        if validation.is_valid {
                            engine.push_conversation_turn(
                                world_state.tick_id,
                                ws_summary,
                                assistant_summary,
                                engine.take_last_reasoning_content(),
                            );
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

                        // 将解析错误注入重试 feedback，让 LLM 知道上次哪里错了。
                        // 格式错误的反馈构造见 retry_feedback_for_error 文档
                        //（技术细节剥离 + 连续失败干净重试）。
                        if is_format_parse_error(&last_error) {
                            format_fail_streak += 1;
                        } else {
                            format_fail_streak = 0;
                        }
                        feedback = retry_feedback_for_error(&last_error, format_fail_streak);

                        // 按统一分类决定是否中止重试
                        // call_with_fallback 已尝试所有可用客户端，继续重试无意义
                        let (action, _reason) = classify_llm_error(&e);
                        match action {
                            ErrorAction::Retry => {
                                // 网络瞬时故障，可能恢复；指数退避后重试，
                                // 避免对 provider / breaker 形成瞬时失败风暴
                                let backoff =
                                    std::time::Duration::from_secs(1u64 << attempt.min(4));
                                warn!(
                                    "[cognitive] Retrying in {:?} (after attempt {})",
                                    backoff,
                                    attempt + 1
                                );
                                tokio::time::sleep(backoff).await;
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

// ============================================================================
// 决策模型两段式路径
// ============================================================================

/// 决策模型意图的 thought_log 来源标注
fn decision_thought(thought_process: &str, confidence: f64) -> String {
    format!("{thought_process}（来源=decision_model conf={confidence:.3}）")
}

/// 决策模型门控结果：Some((intent, chain)) = 采用；None = 门控未过（回退）
#[allow(clippy::too_many_arguments)]
async fn decide_via_model(
    engine: &Arc<CognitiveEngine>,
    model: &Arc<DecisionModelManager>,
    world_state: &WorldState,
    memory_context: &str,
    feedback: Option<&str>,
    soul_cycle_attempt: i32,
) -> anyhow::Result<Option<(Intent, CognitiveChain)>> {
    let started = Instant::now();
    let tick_id = world_state.tick_id;
    let agent_id = world_state.agent_id.unwrap_or_default();

    // 1. 人魂认知-only（LLM 调用；失败即整体回退）
    let cog = engine
        .think_cognition_only(world_state, memory_context, feedback, soul_cycle_attempt)
        .await?;
    dm::metrics::record_cognition(cog.duration_ms);

    // 2. 结构化候选 + 动作词表（WorldState 结构化实体，不用文本正则）
    let candidates = dm::build_candidates(world_state);
    let criteria = dm::action_criteria(&engine.available_actions_snapshot());
    if criteria.is_empty() {
        anyhow::bail!("动作词表为空（game_rules 未下发），决策模型路径不可用");
    }
    let state_text =
        dm::build_state_text(&cog.system_message, &cog.tick_message, &cog.cognition_block);

    // 3. act1（12 选 1）
    let act1_answer = model
        .decide_one(&state_text, &dm::build_act1_question(&criteria))
        .await?;
    dm::metrics::record_act1_confidence(act1_answer.confidence);
    let act1 = act1_answer.choice.clone();
    if !dm::act1_gate_pass(&act1, act1_answer.confidence, model.threshold()) {
        if dm::BINDABLE_ACTIONS.contains(&act1.as_str()) {
            dm::metrics::record_fallback_low_conf();
        } else {
            dm::metrics::record_fallback_ineligible();
        }
        return Ok(None);
    }

    // 4. act2（选项含「无」；一次一问顺序调用）
    let act2_answer = model
        .decide_one(&state_text, &dm::build_act2_question(&criteria))
        .await?;
    let act2 = act2_answer.choice.clone();

    // 5. 实体绑定问题（按第 1 动作语义需要才问；候选为空则不问）
    let item_opts: Vec<(String, String)> = candidates
        .items
        .iter()
        .map(|(display, key, _)| (display.clone(), key.clone()))
        .collect();
    let item_answer = match dm::build_item1_question(&act1, &item_opts) {
        Some(q) => Some(model.decide_one(&state_text, &q).await?.choice),
        None => None,
    };
    let agent_opts: Vec<(String, String)> = candidates
        .agents
        .iter()
        .map(|(id, key)| (id.clone(), key.clone()))
        .collect();
    let agent_answer = match dm::build_agent1_question(&act1, &agent_opts) {
        Some(q) => Some(model.decide_one(&state_text, &q).await?.choice),
        None => None,
    };
    let loc_opts: Vec<(String, String)> = candidates
        .locs
        .iter()
        .map(|(id, key)| (id.clone(), key.clone()))
        .collect();
    let loc_answer = match dm::build_loc1_question(&act1, &loc_opts) {
        Some(q) => Some(model.decide_one(&state_text, &q).await?.choice),
        None => None,
    };

    // 6. 绑定主意图 action_data（绑定失败整体回退——宁走 LLM 不出坏意图）
    let binding = dm::bind_act1(
        &act1,
        item_answer.as_deref(),
        agent_answer.as_deref(),
        loc_answer.as_deref(),
        &candidates,
    );
    if let Some(ref err) = binding.bind_error {
        dm::metrics::record_fallback_ineligible();
        info!(
            "[decision_model] tick {} act1=「{}」绑定失败: {}（回退 LLM 决策路径）",
            tick_id, act1, err
        );
        return Ok(None);
    }

    // 7. 组装 Intents（act2 仅采纳「无/休整/观察」——无需实体绑定）
    let thought1 = decision_thought(&cog_thought(&cog), act1_answer.confidence);
    let mut intents = Vec::new();
    intents.push(
        Intent::new(
            agent_id,
            tick_id,
            binding.action_type.as_str(),
            binding.action_data,
        )
        .with_thought(thought1),
    );
    if dm::act2_gate_pass(&act1, &act2) {
        let thought2 = decision_thought(&cog_thought(&cog), act2_answer.confidence);
        intents.push(Intent::new(agent_id, tick_id, act2.as_str(), None).with_thought(thought2));
    }

    // 8. 构造完整认知链（决策阶段补全 4 stage；天魂照常四层审查）
    let thought_text = cog_thought(&cog);
    let mut chain = cog.chain;
    let primary = &intents[0];
    let decision_content = format!(
        "思考: {}\n决策: {} {:?}{}（决策模型 conf={:.3}）",
        thought_text,
        primary.action_type.as_str(),
        primary.action_data,
        if intents.len() > 1 {
            format!(" (+{} 后续)", intents.len() - 1)
        } else {
            String::new()
        },
        act1_answer.confidence,
    );
    let decision_stage = StageOutput::with_metadata(
        CognitiveStage::Decision,
        decision_content,
        serde_json::json!({
            "source": "decision_model",
            "act1": {
                "choice": act1,
                "confidence": act1_answer.confidence,
                "probabilities": act1_answer.probabilities,
            },
            "act2": {
                "choice": act2,
                "confidence": act2_answer.confidence,
            },
            "bindings": {
                "item": item_answer,
                "agent": agent_answer,
                "loc": loc_answer,
            },
        }),
    );
    chain.add_stage(decision_stage);
    chain.final_intent = intents[0].clone();
    chain.should_remember = cog.should_remember;
    chain.memory_content = cog.memory_content;
    chain.multi_intents = if intents.len() > 1 {
        Some(intents[1..].to_vec())
    } else {
        None
    };
    chain.duration_ms = cog.duration_ms + started.elapsed().as_millis() as u64;

    // 9. 认知链质量校验（与既有路径同口径；不过则回退）
    let validator = CognitiveValidator::new(chain.persona.clone());
    let validation = validator.validate(&chain);
    if !validation.is_valid {
        warn!(
            "[decision_model] tick {} 认知链校验未过: {}（回退 LLM 决策路径）",
            tick_id,
            validation.reason.unwrap_or_default()
        );
        return Ok(None);
    }

    // 10. 对话历史（与既有路径同口径：user=世界摘要，assistant=动作摘要）
    let ws_summary = format!(
        "Tick {} @ {}",
        world_state.tick_id, world_state.location.node_id
    );
    const ASSISTANT_SUMMARY_CHAR_LIMIT: usize = 200;
    let assistant_summary = match primary
        .action_data
        .as_ref()
        .and_then(|d| d.get("content"))
        .and_then(|v| v.as_str())
    {
        Some(content) if !content.is_empty() => format!(
            "{}: {}",
            primary.action_type,
            content
                .chars()
                .filter(|c| !c.is_control())
                .take(ASSISTANT_SUMMARY_CHAR_LIMIT)
                .collect::<String>()
        ),
        _ => primary.action_type.to_string(),
    };
    engine.push_conversation_turn(
        tick_id,
        ws_summary,
        assistant_summary,
        engine.take_last_reasoning_content(),
    );

    info!(
        "[decision_model] tick {} 采用决策输出: {} (+{} 后续), act1_conf={:.3}, 耗时 {}ms",
        tick_id,
        primary.action_type.as_str(),
        intents.len() - 1,
        act1_answer.confidence,
        chain.duration_ms
    );
    Ok(Some((intents[0].clone(), chain)))
}

/// 认知思考过程（决策阶段内容/复用）
fn cog_thought(cog: &CognitionOutput) -> String {
    // cognition_block 已含思考过程行（与训练状态文本同源），直接引用
    cog.cognition_block
        .lines()
        .find(|l| l.starts_with("思考过程: "))
        .map(|l| l["思考过程: ".len()..].to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::retry_feedback_for_error;

    /// 决策来源标注格式
    #[test]
    fn decision_thought_marks_source() {
        let t = super::decision_thought("先找些吃的", 0.941);
        assert!(t.starts_with("先找些吃的"));
        assert!(t.contains("来源=decision_model"));
        assert!(t.contains("conf=0.941"));
    }

    /// 格式解析错误：反馈不携带 serde 技术细节原文。
    #[test]
    fn test_format_error_feedback_strips_technical_detail() {
        let fb = retry_feedback_for_error(
            "invalid type: null, expected a string at line 1 column 232",
            1,
        )
        .expect("首次格式失败应保留反馈");
        assert!(!fb.contains("invalid type"), "不得携带错误原文");
        assert!(!fb.contains("column"), "不得携带定位细节");
        assert!(fb.contains("格式有误"), "保留格式有误信号");
    }

    /// 连续第 2 次格式失败起：反馈清空，干净重试。
    #[test]
    fn test_format_error_streak_clears_feedback() {
        let fb = retry_feedback_for_error("expected value at line 1 column 1", 2);
        assert!(fb.is_none(), "连续 2 次格式失败后应干净重试");
        let fb = retry_feedback_for_error("missing field `actions`", 3);
        assert!(fb.is_none());
    }

    /// 非格式错误：维持原行为（携带错误原文）。
    #[test]
    fn test_non_format_error_keeps_detail() {
        let fb = retry_feedback_for_error("error sending request for url ...", 0)
            .expect("非格式错误应保留反馈");
        assert!(fb.contains("error sending request"), "网络错误保留原文");
    }

    /// 格式失败计数被非格式错误重置：混合序列下污染阻断仍生效。
    #[test]
    fn test_streak_semantics() {
        assert!(retry_feedback_for_error("invalid type: null", 1).is_some());
        assert!(retry_feedback_for_error("invalid type: null", 2).is_none());
        assert!(retry_feedback_for_error("timeout", 0).is_some());
    }
}
