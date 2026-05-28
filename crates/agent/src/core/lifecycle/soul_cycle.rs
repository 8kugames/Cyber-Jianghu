// ============================================================================
// 三魂循环：ActorSoul → ReflectorSoul 审查 + 后置处理
// ============================================================================
//
// 核心决策循环：
//   ActorSoul 产出 Intent → ReflectorSoul 审查 → self-correct → chaos fallback
//   + 后置处理：认知失败替换、LLM 失败追踪、intent 历史
//
// 调用路径: run() → run_three_soul_cycle() → (final_intent, was_validated)
// ============================================================================

use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use cyber_jianghu_protocol::WorldState;
use tracing::{info, warn};

use crate::component::memory::backend::MemoryBackend;
use crate::models::Intent;

/// 三魂循环输出
pub(crate) struct SoulCycleResult {
    pub intent: Intent,
    pub validated: bool,
}

impl super::super::Agent {
    /// 执行三魂循环 + 后置处理
    ///
    /// 流程：
    /// 1. 根据 token_optimization 配置确定重试策略
    /// 2. for attempt 循环：ActorSoul 决策 → ReflectorSoul 审查 → self-correct
    /// 3. 后置：chaos 替换认知失败休息、LLM 失败追踪、intent 历史记录
    pub(crate) async fn run_three_soul_cycle(
        &mut self,
        world_state: &WorldState,
        memory_context: &str,
        active_dream: Option<&str>,
        last_intents_for_narrative: &Arc<Mutex<Vec<Intent>>>,
    ) -> Result<SoulCycleResult> {
        // 提前提取优化配置（避免后续 borrow 冲突）
        let (opt_enabled, opt_self_correction, opt_chaos_on_double_reject, opt_chaos_on_llm_fail) = {
            let opt = &self.config.token_optimization;
            (
                opt.enabled,
                opt.reflector.self_correction,
                opt.reflector.chaos_on_double_reject,
                opt.reflector.chaos_on_llm_fail,
            )
        };
        let max_retries: i32 = if opt_enabled {
            // 优化模式：最多 self_correct 一次（attempt 0=初始, 1=纠正）
            1
        } else {
            // 旧模式：保留原有重试上限
            self.config
                .game_rules
                .as_ref()
                .and_then(|g| g.intent_batch.as_ref())
                .map(|b| b.max_retries)
                .unwrap_or(12)
        };
        let _max_intents = self
            .config
            .game_rules
            .as_ref()
            .and_then(|g| g.intent_batch.as_ref())
            .map(|b| b.max_intents_per_tick)
            .unwrap_or(5);
        let agent_id = world_state.agent_id.unwrap_or_default();
        let mut final_intent = None;
        let mut final_intent_validated = false;

        // tick 级 LLM 失败计数器（优化模式下使用）
        let mut tick_llm_fail_count: u32 = 0;

        // 注入对话上下文到 CognitiveEngine（供 build_prompt 的 {dialogue_section} 使用）
        if let Some(ref engine) = self.cognitive_engine {
            let dialogue_ctx = if let Some(ref dm) = self.dialogue_manager {
                let guard = dm.read().await;
                guard.get_active_sessions_context()
            } else {
                String::new()
            };
            engine.set_dialogue_context(dialogue_ctx);
        }

        for attempt in 0..=max_retries {
            // 5a. 人魂 (ActorSoul) 决策 — 直连 WorldState，输出结构化 Intent
            let (raw_intent, cognitive_chain) = {
                let tick_id = world_state.tick_id;
                let agent_id = world_state.agent_id.unwrap_or_default();
                let decision_future = async {
                    // 最高优先级：decision_with_chain_callback（人魂直连 WorldState）
                    if let Some(ref chain_callback) = self.decision_with_chain_callback {
                        let fb = self.last_rejection_reason.as_deref();
                        return chain_callback(world_state, memory_context, fb).await;
                    }

                    // 降级路径：旧式回调（不接收 WorldState）
                    if let Some(ref reason) = self.last_rejection_reason {
                        if let Some(ref callback) = self.decision_with_feedback_callback {
                            let intent =
                                callback(tick_id, agent_id, memory_context, Some(reason.as_str()))
                                    .await;
                            (intent, None)
                        } else if let Some(ref memory_callback) = self.decision_with_memory_callback
                        {
                            let combined = if memory_context.is_empty() {
                                format!("[意图被驳回: {}，请重新决策]", reason)
                            } else {
                                format!("{}\n[意图被驳回: {}，请重新决策]", memory_context, reason)
                            };
                            let intent = memory_callback(tick_id, agent_id, &combined).await;
                            (intent, None)
                        } else {
                            let intent = (self.decision_callback)(tick_id, agent_id).await;
                            (intent, None)
                        }
                    } else if let Some(ref memory_callback) = self.decision_with_memory_callback {
                        let intent = memory_callback(tick_id, agent_id, memory_context).await;
                        (intent, None)
                    } else {
                        let intent = (self.decision_callback)(tick_id, agent_id).await;
                        (intent, None)
                    }
                };

                decision_future.await
            };

            // 如果 final_intent 已被设置（如 speak 即时通道），退出
            if final_intent.is_some() {
                break;
            }

            // 预计算人魂叙述（审查通过后才记录）
            let renhun_narrative = Self::summarize_intent(
                raw_intent.action_type.as_str(),
                raw_intent.action_data.as_ref(),
                &world_state.location.name,
                &world_state.entities,
            );
            let renhun_thought_log = raw_intent.thought_log.as_deref().unwrap_or("");

            // 5c. 天魂 (ReflectorSoul) 审核 — 分级审核策略
            let graded_config = self
                .config
                .game_rules
                .as_ref()
                .and_then(|g| g.intent_batch.as_ref())
                .map(|b| b.llm_validation.clone());

            let mut approved_intents = Vec::new();
            let mut batch_rejection: Option<String> = None;
            let mut batch_layers: Vec<crate::soul::reflector::LayerResult> = Vec::new();
            let mut batch_narrative: Option<String> = None;

            // multi-intent pipeline: primary + subsequent intents + chaos
            let max_per_tick = _max_intents;
            let mut all_raw_intents: Vec<Intent> = {
                let mut intents: Vec<Intent> = if self.llm_chaos_active {
                    Vec::new()
                } else {
                    vec![raw_intent.clone()]
                };
                if let Some(ref chain) = cognitive_chain
                    && let Some(ref multi) = chain.multi_intents
                {
                    for i in multi.iter().take(max_per_tick.saturating_sub(1)) {
                        intents.push(i.clone());
                    }
                }
                if let Some(ref mut generator) = self.chaos_generator {
                    let remaining = max_per_tick.saturating_sub(intents.len());
                    if remaining > 0 {
                        let actions: Vec<_> = self
                            .config
                            .game_rules
                            .as_ref()
                            .map(|g| g.available_actions.clone())
                            .unwrap_or_default();
                        let chaos_intents =
                            generator.generate_chaos_intents(world_state, &actions, remaining);
                        intents.extend(chaos_intents);
                    }
                }
                if self.llm_chaos_active
                    && let Some(ref mut generator) = self.chaos_generator
                {
                    let remaining = max_per_tick.saturating_sub(intents.len());
                    if remaining > 0 {
                        let actions: Vec<_> = self
                            .config
                            .game_rules
                            .as_ref()
                            .map(|g| g.available_actions.clone())
                            .unwrap_or_default();
                        let llm_chaos = generator.generate_llm_chaos_intents(
                            world_state,
                            &actions,
                            remaining,
                            self.consecutive_llm_failures as usize,
                        );
                        tracing::info!(
                            "LLM chaos: generated {} intents from {} actions",
                            llm_chaos.len(),
                            actions.len()
                        );
                        intents.extend(llm_chaos);
                    }
                }
                intents
            };

            // 托梦标记
            if let Some(dream) = active_dream {
                let summary = dream.to_string();
                for intent in &mut all_raw_intents {
                    intent.dream_marker = Some(cyber_jianghu_protocol::types::DreamMarker {
                        thought: summary.clone(),
                    });
                }
            }

            // 重要记忆固化
            #[allow(clippy::collapsible_if)]
            if let Some(ref chain) = cognitive_chain
                && chain.should_remember == Some(true)
                && let Some(ref content) = chain.memory_content
                && let Some(ref mm) = self.memory_manager
            {
                let entry = crate::component::memory::types::MemoryEntry::new(
                    world_state.agent_id.unwrap_or_default(),
                    world_state.tick_id,
                    content.clone(),
                )
                .with_importance(1.0);
                let mut mm_guard = mm.write().await;
                if let Err(e) = mm_guard.episodic_mut().add(&mut entry.clone()).await {
                    warn!("重要记忆固化失败: {}", e);
                } else {
                    info!("重要记忆已固化: {}", content);
                }
            }

            // 逐 intent 审查 + self-correction（优化模式）
            for intent in all_raw_intents {
                let intent_for_summary = intent.clone();
                match self
                    .validate_with_reflector(intent, world_state, graded_config.as_ref())
                    .await?
                {
                    crate::soul::reflector::PipelineValidationResult::Approved {
                        intent: approved,
                        layers,
                        narrative,
                    } => {
                        // 审查通过后推入 summary window（validated=true）
                        if let Some(ref chain) = cognitive_chain
                            && let Some(ref engine) = self.cognitive_engine
                        {
                            engine.push_summary_to_window(chain, &approved, true);
                        }
                        batch_layers = layers;
                        batch_narrative = narrative;
                        approved_intents.push(approved);
                    }
                    crate::soul::reflector::PipelineValidationResult::Rejected {
                        reason,
                        layers,
                    } => {
                        // 驳回的 intent 记录到 action_history（validated=false）
                        if let Some(ref chain) = cognitive_chain
                            && let Some(ref engine) = self.cognitive_engine
                        {
                            engine.push_summary_to_window(chain, &intent_for_summary, false);
                        }
                        batch_layers = layers;
                        let rejection_reason = reason.clone();
                        self.set_rejection_feedback(reason.clone());
                        warn!(
                            "Tick {} attempt {} 天魂审查驳回: {}",
                            world_state.tick_id, attempt, rejection_reason
                        );

                        // 优化模式：self-correct 一次后直接 chaos_fallback
                        if opt_enabled
                            && opt_self_correction
                            && tick_llm_fail_count < opt_chaos_on_llm_fail
                        {
                            match self
                                .self_correct_intent(world_state, memory_context, &rejection_reason)
                                .await
                            {
                                Ok(corrected_intent) => {
                                    match self
                                        .validate_with_reflector(
                                            corrected_intent,
                                            world_state,
                                            graded_config.as_ref(),
                                        )
                                        .await?
                                    {
                                        crate::soul::reflector::PipelineValidationResult::Approved {
                                            intent: approved,
                                            layers: l2,
                                            narrative: n2,
                                        } => {
                                            // self-correct 审查通过后推入 summary window
                                            if let Some(ref chain) = cognitive_chain
                                                && let Some(ref engine) = self.cognitive_engine
                                            {
                                                engine.push_summary_to_window(chain, &approved, true);
                                            }
                                            batch_layers = l2;
                                            batch_narrative = n2;
                                            approved_intents.push(approved);
                                        }
                                        crate::soul::reflector::PipelineValidationResult::Rejected {
                                            reason: reason2,
                                            ..
                                        } => {
                                            warn!(
                                                "Tick {} self-correct 后仍被驳回: {}",
                                                world_state.tick_id, reason2
                                            );
                                            if opt_chaos_on_double_reject {
                                                approved_intents.push(self.chaos_fallback_intent(
                                                    world_state,
                                                    agent_id,
                                                    format!("self-correct 后仍被驳回: {}", reason2),
                                                ));
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    tick_llm_fail_count += 1;
                                    warn!(
                                        "Tick {} self-correct LLM 失败 ({}): {}",
                                        world_state.tick_id, tick_llm_fail_count, e
                                    );
                                    approved_intents.push(self.chaos_fallback_intent(
                                        world_state,
                                        agent_id,
                                        format!("self-correct LLM 失败: {}", e),
                                    ));
                                }
                            }
                        } else if opt_enabled && opt_chaos_on_double_reject {
                            approved_intents.push(self.chaos_fallback_intent(
                                world_state,
                                agent_id,
                                format!("意图被驳回（跳过 self-correct）: {}", rejection_reason),
                            ));
                        } else {
                            // 旧模式：记录 batch_rejection 以触发重试
                            batch_rejection = Some(rejection_reason);
                        }
                    }
                }

                // 旧模式：primary intent 被驳回则终止批次（Pipeline 语义）
                if !opt_enabled && batch_rejection.is_some() {
                    break;
                }
            }

            if !approved_intents.is_empty() {
                if let Some(recorder) = self.soul_recorder().await {
                    recorder
                        .record_renhun(
                            world_state.tick_id,
                            attempt,
                            &renhun_narrative,
                            renhun_thought_log,
                        )
                        .await;
                    let world_time_str = Self::format_world_time(&world_state.world_time);
                    recorder
                        .record_world_time(world_state.tick_id, attempt, &world_time_str)
                        .await;
                    let layer1 = batch_layers.iter().find(|l| l.layer == "layer1");
                    let layer2 = batch_layers.iter().find(|l| l.layer == "layer2");
                    let layer3 = batch_layers.iter().find(|l| l.layer == "layer3");
                    recorder
                        .record_tianhun(
                            world_state.tick_id,
                            attempt,
                            "approved",
                            layer1.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            layer2.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            layer3.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            None,
                            batch_narrative.as_deref(),
                        )
                        .await;
                    let pipeline = Self::assemble_pipeline(approved_intents.clone());
                    recorder
                        .record_final_intent(
                            world_state.tick_id,
                            attempt,
                            Some(&pipeline.intent_id.to_string()),
                            Some(pipeline.action_type.as_str()),
                            pipeline
                                .action_data
                                .as_ref()
                                .map(|d| serde_json::to_string(d).unwrap_or_default())
                                .as_deref(),
                        )
                        .await;
                    final_intent = Some(pipeline);
                    final_intent_validated = true;
                } else {
                    let pipeline = Self::assemble_pipeline(approved_intents.clone());
                    final_intent = Some(pipeline);
                    final_intent_validated = true;
                }
                if let Ok(mut saved) = last_intents_for_narrative.lock() {
                    saved.clone_from(&approved_intents);
                } else {
                    warn!("暂存 approved_intents 失败：Mutex lock 获取失败");
                }
                break;
            } else if let Some(reason) = batch_rejection.clone() {
                // 仅旧模式会进入此分支
                if let Some(recorder) = self.soul_recorder().await {
                    let layer1 = batch_layers.iter().find(|l| l.layer == "layer1");
                    let layer2 = batch_layers.iter().find(|l| l.layer == "layer2");
                    let layer3 = batch_layers.iter().find(|l| l.layer == "layer3");
                    let narrated = super::super::Agent::narrativize_rejection(&reason);
                    recorder
                        .record_tianhun(
                            world_state.tick_id,
                            attempt,
                            "rejected",
                            layer1.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            layer2.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            layer3.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            Some(&reason),
                            Some(&narrated),
                        )
                        .await;
                }

                if attempt >= max_retries {
                    warn!(
                        "Tick {} 达到最大重试次数 {}，使用 chaos fallback",
                        world_state.tick_id, max_retries
                    );
                    final_intent = Some(self.chaos_fallback_intent(
                        world_state,
                        agent_id,
                        format!("意图多次被驳回: {}", reason),
                    ));
                    break;
                }
            }
        }

        let mut final_intent = match final_intent {
            Some(intent) => intent,
            None => {
                warn!(
                    "Tick {} 无有效 intent（超时或被驳回耗尽），使用 chaos fallback",
                    world_state.tick_id
                );
                self.consecutive_idle_count += 1;
                self.maybe_rotate_model().await;
                self.chaos_fallback_intent(
                    world_state,
                    agent_id,
                    "三魂循环未产出有效意图".to_string(),
                )
            }
        };

        // 后置 chaos 替换：认知失败标记的休息 → chaos 生存 intent
        // 避免"认知失败 → 固定休息 → 饿死"死循环
        if final_intent.action_type.as_str() == "休息"
            && final_intent
                .thought_log
                .as_ref()
                .map(|t| t.contains("认知失败") || t.contains("忽然心神不宁"))
                .unwrap_or(false)
        {
            let chaos_intent = self.chaos_fallback_intent(
                world_state,
                agent_id,
                final_intent.thought_log.clone().unwrap_or_default(),
            );
            info!(
                "认知失败休息 → chaos 替换: action={}",
                chaos_intent.action_type
            );
            final_intent = chaos_intent;
        }

        // LLM 失败追踪
        let is_llm_failure = final_intent.chaos_marker.is_some()
            || final_intent
                .thought_log
                .as_ref()
                .map(|t| {
                    t.contains("意图多次被驳回")
                        || t.contains("三魂循环未产出有效意图")
                        || t.contains("认知失败")
                        || t.contains("[LLM 配额耗尽")
                })
                .unwrap_or(false);
        if is_llm_failure {
            self.consecutive_llm_failures += 1;
        } else {
            self.consecutive_llm_failures = 0;
        }
        let llm_chaos_threshold = self
            .config
            .game_rules
            .as_ref()
            .and_then(|g| g.intent_batch.as_ref())
            .map(|b| b.llm_chaos_threshold)
            .unwrap_or(12);
        let was_chaos_active = self.llm_chaos_active;
        self.llm_chaos_active = self.consecutive_llm_failures >= llm_chaos_threshold;
        if self.llm_chaos_active && !was_chaos_active {
            warn!(
                "LLM chaos 模式激活: agent={}, consecutive_failures={}",
                self.character_name(),
                self.consecutive_llm_failures
            );

            // 连续失败达到 chaos 阈值时，主动轮换模型（避免 sticky 到坏模型无法恢复）
            if let Some(ref container) = self.actor_llm_container {
                let llm = container.read().await;
                if llm.force_rotate_model() {
                    warn!(
                        "LLM 连续失败 {} 次，主动轮换模型（agent={}）",
                        self.consecutive_llm_failures,
                        self.character_name(),
                    );
                    let new_tokens = llm.context_window_tokens() as usize;
                    drop(llm);
                    if let Some(ref engine) = self.cognitive_engine {
                        engine.update_conversation_max_tokens(new_tokens);
                    }
                }
            }
        } else if !self.llm_chaos_active && was_chaos_active {
            info!(
                "LLM chaos 模式解除: agent={}, LLM 恢复正常",
                self.character_name()
            );
        }

        // 记录 Intent 到经历日志（供 Web Panel 查询）
        if let Some(ref api_state) = self.http_api_state
            && let Some(history) = api_state.intent_history.read().await.as_ref()
        {
            history
                .record_intent(
                    final_intent.tick_id,
                    0,
                    final_intent.intent_id,
                    final_intent.action_type.to_string(),
                    final_intent.thought_log.clone(),
                )
                .await;
        }

        Ok(SoulCycleResult {
            intent: final_intent,
            validated: final_intent_validated,
        })
    }
}
