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

/// 记忆通道元游戏术语黑名单（与天魂 Layer3 绝对禁止项同源：soul/reflector/prompt.rs）。
/// ASCII 词为大小写敏感子串匹配（大写 "MP" 不误伤 camp/sample 等英文小写词），中文词直接包含匹配。
const MEMORY_OOC_ASCII_TERMS: &[&str] = &["HP", "SAN", "MP", "NPC"];
const MEMORY_OOC_CN_TERMS: &[&str] = &[
    "玩家",
    "血量",
    "数值",
    "属性栏",
    "状态栏",
    "登录",
    "存档",
    "复活",
    "经验值",
    "账号",
    "充值",
    "服务器",
    "上线",
    "掉线",
    "版本",
    "补丁",
];

/// 判定文本是否含元游戏术语（元术语不得经由记忆通道进入世界模型）
fn contains_meta_game_term(text: &str) -> bool {
    MEMORY_OOC_ASCII_TERMS.iter().any(|t| text.contains(t))
        || MEMORY_OOC_CN_TERMS.iter().any(|t| text.contains(t))
}

/// 三魂循环输出
pub(crate) struct SoulCycleResult {
    pub intent: Intent,
    pub validated: bool,
    /// 最后一次尝试的序号（调用方做 chaos 覆写留痕时定位记录行）
    pub attempt: i32,
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
        // 最后一次尝试序号：后置 chaos 覆写留痕时定位记录行
        let mut last_attempt: i32 = 0;

        // tick 级 LLM 失败计数器（优化模式下使用）
        let mut tick_llm_fail_count: u32 = 0;

        // 注入对话上下文到 CognitiveEngine（供 build_tick_message 的 {dialogue_section} 使用）
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
            last_attempt = attempt;
            // 5a. 人魂 (ActorSoul) 决策 — 直连 WorldState，输出结构化 Intent
            let (raw_intent, cognitive_chain) = {
                let tick_id = world_state.tick_id;
                let agent_id = world_state.agent_id.unwrap_or_default();
                let decision_future = async {
                    // 最高优先级：decision_with_chain_callback（人魂直连 WorldState）
                    if let Some(ref chain_callback) = self.decision_with_chain_callback {
                        let fb = self.last_rejection_reason.as_deref();
                        return chain_callback(world_state, memory_context, fb, attempt).await;
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

            // 5c. 天魂 (ReflectorSoul) 审核 — 分级审核策略
            let graded_config = self
                .config
                .game_rules
                .as_ref()
                .and_then(|g| g.intent_batch.as_ref())
                .map(|b| b.llm_validation.clone());

            let mut approved_intents = Vec::new();
            let mut intent_verdicts: Vec<String> = Vec::new();
            let mut batch_rejection: Option<String> = None;
            let mut batch_layers: Vec<crate::soul::reflector::LayerResult> = Vec::new();
            // 多意图逐意图审查结果聚合（按送审顺序）。
            // 历史 bug：循环内 batch_layers 覆盖式赋值，仅末意图的天魂层被记录，
            // 前序意图（如说话的 LLM 审查）结果丢失、UI 无从追溯。
            let mut per_intent_layers: Vec<(String, Vec<crate::soul::reflector::LayerResult>)> =
                Vec::new();
            let mut used_chaos_fallback = false;
            // 链内前序已验证"取"动作获得的物品（裸 item_id）：
            // 后序"取后即用/予"类 intent 共享同一 WorldState 快照，
            // 需将获得物并入可见集合以免误拦合法连续动作
            let mut acquired_item_ids: Vec<String> = Vec::new();
            // chaos 替补意图缓冲：intent 循环结束后统一追加到 approved_intents
            // 队尾（不顶替被驳回的主槽——服务端主意图失败即中止整个队列，
            // chaos 占主槽会在其执行失败时连坐后续已通过的合法意图）
            let mut pending_chaos: Vec<Intent> = Vec::new();

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

            // 人魂叙述：基于全部原始意图（审查前），体现角色主观意图
            let renhun_narrative = all_raw_intents
                .iter()
                .map(|intent| {
                    Self::summarize_intent(
                        intent.action_type.as_str(),
                        intent.action_data.as_ref(),
                        &world_state.location.name,
                        &world_state.entities,
                    )
                })
                .collect::<Vec<_>>()
                .join("；");
            let renhun_thought_log = raw_intent.thought_log.as_deref().unwrap_or("");

            // 重要记忆固化（入库前元游戏术语过滤，同天魂绝对禁止项：
            // "温九辞被记忆标成 NPC"的客户端侧兜底，模板修正为根因修复）
            if let Some(ref chain) = cognitive_chain
                && chain.should_remember == Some(true)
                && let Some(ref content) = chain.memory_content
            {
                if contains_meta_game_term(content) {
                    warn!("重要记忆含元游戏术语，拒绝入库: {}", content);
                } else if let Some(ref mm) = self.memory_manager {
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
            }

            // 人魂决策完成，立即记录该 attempt 的人魂输出 + 地魂 tool call（天魂审查前）
            // model_id 取该次人魂实际使用的 LLM 模型（含降级后真实模型），写入经历日志。
            let attempt_model_id = if let Some(ref container) = self.actor_llm_container {
                let llm = container.read().await;
                llm.model_name()
            } else {
                String::new()
            };
            if let Some(recorder) = self.soul_recorder().await {
                recorder
                    .record_renhun(
                        world_state.tick_id,
                        attempt,
                        &renhun_narrative,
                        renhun_thought_log,
                        &attempt_model_id,
                    )
                    .await;
                if let Some(ref engine) = self.cognitive_engine
                    && let Some(tool_calls) = engine.take_last_tool_call_log()
                    && !tool_calls.is_empty()
                    && let Ok(json) = serde_json::to_string(&tool_calls)
                {
                    recorder
                        .record_earth_tool_calls(world_state.tick_id, attempt, &json)
                        .await;
                }
                let world_time_str = Self::format_world_time(&world_state.world_time);
                recorder
                    .record_world_time(world_state.tick_id, attempt, &world_time_str)
                    .await;
            }

            // 逐 intent 审查 + self-correction（优化模式）
            // 移动目标规范化已下沉到 validate_pipeline（normalize 块后单一收口点，
            // 覆盖 raw/自纠正/别名/Claw/HTTP 全入口）
            for (intent_idx, intent) in all_raw_intents.into_iter().enumerate() {
                let intent_action_label = intent.action_type.as_str().to_string();
                let intent_for_summary = intent.clone();
                match self
                    .validate_with_reflector(
                        intent,
                        world_state,
                        graded_config.as_ref(),
                        acquired_item_ids.clone(),
                    )
                    .await?
                {
                    crate::soul::reflector::PipelineValidationResult::Approved {
                        intent: approved,
                        layers,
                        narrative: _,
                    } => {
                        // 链感知：取动作 approved 后其目标物品（已规范化为完整 uuid）
                        // 供同链后序 intent 的 layer0 可见性检查（可见集合为 uuid 形态）
                        if approved.action_type.as_str() == "取"
                            && let Some(item_id) = approved
                                .action_data
                                .as_ref()
                                .and_then(|d| d.get("item_id"))
                                .and_then(|v| v.as_str())
                        {
                            acquired_item_ids.push(item_id.to_string());
                        }
                        // 审查通过后推入 summary window（validated=true）
                        if let Some(ref chain) = cognitive_chain
                            && let Some(ref engine) = self.cognitive_engine
                        {
                            engine.push_summary_to_window(chain, &approved, true);

                            // 回写 LLM 构造的情绪到 persona
                            if let Some(emotion) = engine.take_constructed_emotion()
                                && !emotion.label.is_empty()
                            {
                                engine.update_persona_emotion(emotion.label.clone());
                                // 情绪强度 → 特质 delta（intensity * trait_intensity_scale）
                                if let Some(ref emotion_config) = self.emotion_config {
                                    let trait_delta = (emotion.intensity
                                        * emotion_config.core_affect.trait_intensity_scale)
                                        as i16;
                                    engine.apply_persona_trait_change(
                                        &emotion.label,
                                        trait_delta,
                                        emotion.reasoning.clone(),
                                        world_state.tick_id,
                                    );
                                }
                            }
                        }
                        batch_layers = layers.clone();
                        per_intent_layers.push((intent_action_label.clone(), layers));
                        approved_intents.push(approved);
                        intent_verdicts.push(format!(
                            "第{}项[{}]通过",
                            intent_idx + 1,
                            intent_action_label
                        ));
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
                        batch_layers = layers.clone();
                        per_intent_layers.push((intent_action_label.clone(), layers));
                        intent_verdicts.push(format!(
                            "第{}项[{}]驳回",
                            intent_idx + 1,
                            intent_action_label
                        ));
                        // 反馈明确标记意图组内逐项通过/驳回结果，供 self-correction 精准纠错
                        let marked_feedback = format!(
                            "意图组审查结果：{}；驳回详情：{}",
                            intent_verdicts.join("；"),
                            reason
                        );
                        let rejection_reason = marked_feedback;
                        self.set_rejection_feedback(rejection_reason.clone());
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
                                .self_correct_intent(
                                    world_state,
                                    memory_context,
                                    &rejection_reason,
                                    attempt,
                                )
                                .await
                            {
                                Ok(corrected_intent) => {
                                    // 自纠是完整重决策（可更换动作类型），标签用纠正后
                                    // 真实动作名，否则出现「取(自纠)」实际执行「用」的
                                    // 面板矛盾
                                    let corrected_label =
                                        corrected_intent.action_type.as_str().to_string();
                                    match self
                                        .validate_with_reflector(
                                            corrected_intent,
                                            world_state,
                                            graded_config.as_ref(),
                                            acquired_item_ids.clone(),
                                        )
                                        .await?
                                    {
                                        crate::soul::reflector::PipelineValidationResult::Approved {
                                            intent: approved,
                                            layers: l2,
                                            narrative: _,
                                        } => {
                                            // 链感知：自纠产出的「取」过审后同样并入获得物
                                            // 集合（与主循环对齐，否则收窄后的 layer0 会
                                            // 误拦「自纠取→后续用/予」链）
                                            if approved.action_type.as_str() == "取"
                                                && let Some(item_id) = approved
                                                    .action_data
                                                    .as_ref()
                                                    .and_then(|d| d.get("item_id"))
                                                    .and_then(|v| v.as_str())
                                            {
                                                acquired_item_ids.push(item_id.to_string());
                                            }
                                            // self-correct 审查通过后推入 summary window
                                            if let Some(ref chain) = cognitive_chain
                                                && let Some(ref engine) = self.cognitive_engine
                                            {
                                                engine.push_summary_to_window(chain, &approved, true);
                                            }
                                            batch_layers = l2.clone();
                                            per_intent_layers.push((
                                                format!("{}(自纠)", corrected_label),
                                                l2,
                                            ));
                                            approved_intents.push(approved);
                                        }
                                        crate::soul::reflector::PipelineValidationResult::Rejected {
                                            reason: reason2,
                                            layers: l2,
                                        } => {
                                            warn!(
                                                "Tick {} self-correct 后仍被驳回: {}",
                                                world_state.tick_id, reason2
                                            );
                                            // 自纠尝试自身留痕（此前该分支零记录，
                                            // 面板无从追溯纠正路径）
                                            per_intent_layers.push((
                                                format!("{}(自纠·驳回)", corrected_label),
                                                l2,
                                            ));
                                            if opt_chaos_on_double_reject {
                                                used_chaos_fallback = true;
                                                pending_chaos.push(self.chaos_fallback_intent(
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
                                    // 自纠 LLM 失败同样留痕（无审查层结果，仅意图标签）
                                    per_intent_layers.push((
                                        format!("{}(自纠·LLM失败)", intent_action_label),
                                        Vec::new(),
                                    ));
                                    used_chaos_fallback = true;
                                    pending_chaos.push(self.chaos_fallback_intent(
                                        world_state,
                                        agent_id,
                                        format!("self-correct LLM 失败: {}", e),
                                    ));
                                }
                            }
                        } else if opt_enabled && opt_chaos_on_double_reject {
                            used_chaos_fallback = true;
                            pending_chaos.push(self.chaos_fallback_intent(
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

            // chaos 沉队尾：合法已审意图优先执行，chaos 永不占主槽（主槽失败会
            // 连坐整个队列，chaos 沉尾后即使失败也无后续损失）。
            // 纯驳回 tick 依赖 chaos 保证 approved_intents 非空（落库 + 组 pipeline），
            // 故必须在下方 is_empty 门控之前并入。chaos 意图不经天魂审查，
            // 留痕为「动作(chaos)」空层条目以保留地魂动作的来历可追溯性。
            for chaos_intent in pending_chaos {
                per_intent_layers.push((
                    format!("{}(chaos)", chaos_intent.action_type.as_str()),
                    Vec::new(),
                ));
                approved_intents.push(chaos_intent);
            }

            // 聚合 JSON：[{"intent":"吃","layers":[{layer,passed,detail}]}]
            // 由 recorder 落入 tianhun_layers 列，handler/上报链解析后逐意图展示。
            // approved / rejected 两条 record_tianhun 路径共用。
            let per_intent_json: Vec<serde_json::Value> = per_intent_layers
                .iter()
                .map(|(label, ls)| {
                    serde_json::json!({
                        "intent": label,
                        "layers": ls.iter().map(|l| serde_json::json!({
                            "layer": l.layer,
                            "passed": l.passed,
                            "detail": l.detail,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let layers_json = serde_json::to_string(&per_intent_json).ok();

            if !approved_intents.is_empty() {
                if let Some(recorder) = self.soul_recorder().await {
                    let layer0 = batch_layers.iter().find(|l| l.layer == "layer0");
                    let layer1 = batch_layers.iter().find(|l| l.layer == "layer1");
                    let layer2 = batch_layers.iter().find(|l| l.layer == "layer2");
                    let layer3 = batch_layers.iter().find(|l| l.layer == "layer3");
                    let tianhun_result = if used_chaos_fallback {
                        "chaos_fallback"
                    } else {
                        "approved"
                    };
                    recorder
                        .record_tianhun(
                            world_state.tick_id,
                            attempt,
                            tianhun_result,
                            layer0.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            layer1.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            layer2.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            layer3.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            if used_chaos_fallback {
                                Some("天魂审查未通过，使用 chaos fallback")
                            } else {
                                None
                            },
                            layers_json.as_deref(),
                        )
                        .await;
                    let pipeline = Self::assemble_pipeline(approved_intents.clone());
                    // 构建 pipeline 完整视图：primary + subsequent intents
                    let pipeline_actions: Vec<serde_json::Value> =
                        std::iter::once(serde_json::json!({
                            "action_type": pipeline.action_type,
                            "action_data": pipeline.action_data,
                            "intent_id": pipeline.intent_id,
                        }))
                        .chain(pipeline.subsequent_intents.iter().map(|si| {
                            serde_json::json!({
                                "action_type": si.action_type,
                                "action_data": si.action_data,
                                "intent_id": si.intent_id,
                            })
                        }))
                        .collect();
                    let primary_action_data = pipeline
                        .action_data
                        .as_ref()
                        .and_then(|d| serde_json::to_string(d).ok());
                    let pipeline_json = serde_json::to_string(&pipeline_actions).ok();
                    recorder
                        .record_final_intent(
                            world_state.tick_id,
                            attempt,
                            Some(&pipeline.intent_id.to_string()),
                            Some(pipeline.action_type.as_str()),
                            primary_action_data.as_deref(),
                            pipeline_json.as_deref(),
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
                    let layer0 = batch_layers.iter().find(|l| l.layer == "layer0");
                    let layer1 = batch_layers.iter().find(|l| l.layer == "layer1");
                    let layer2 = batch_layers.iter().find(|l| l.layer == "layer2");
                    let layer3 = batch_layers.iter().find(|l| l.layer == "layer3");
                    recorder
                        .record_tianhun(
                            world_state.tick_id,
                            attempt,
                            "rejected",
                            layer0.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            layer1.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            layer2.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            layer3.map(|l| l.detail.as_deref().unwrap_or("通过")),
                            Some(&reason),
                            layers_json.as_deref(),
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
        if final_intent.action_type.as_str() == "休整"
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
            // 留痕：final intent 已按原（休整）pipeline 落库，覆写为实际发送的
            // chaos 意图，并在天魂理由追加替换说明（否则面板记录与执行不符）
            self.record_chaos_override(
                world_state.tick_id,
                last_attempt,
                &chaos_intent,
                &format!(
                    "认知失败休息已被 chaos 替换为: {}",
                    chaos_intent.action_type.as_str()
                ),
            )
            .await;
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

        Ok(SoulCycleResult {
            intent: final_intent,
            validated: final_intent_validated,
            attempt: last_attempt,
        })
    }

    /// chaos 覆写留痕：后置替换发生时，既有记录要么已按原 pipeline 落库
    /// （认知失败休整替换）、要么根本没有 final intent 记录（fallback 被驳回
    /// 后的二次 chaos）。此处将实际发送的 chaos 意图覆写进 final intent 列，
    /// 并在天魂理由追加替换说明——保证面板「地魂」与实际执行一致、「天魂」
    /// 可追溯替换缘由。
    pub(crate) async fn record_chaos_override(
        &self,
        tick_id: i64,
        attempt: i32,
        intent: &Intent,
        note: &str,
    ) {
        let Some(recorder) = self.soul_recorder().await else {
            return;
        };
        let pipeline = Self::assemble_pipeline(vec![intent.clone()]);
        let action_data = pipeline
            .action_data
            .as_ref()
            .and_then(|d| serde_json::to_string(d).ok());
        let pipeline_actions = vec![serde_json::json!({
            "action_type": pipeline.action_type,
            "action_data": pipeline.action_data,
            "intent_id": pipeline.intent_id,
        })];
        let pipeline_json = serde_json::to_string(&pipeline_actions).ok();
        recorder
            .record_final_intent(
                tick_id,
                attempt,
                Some(&pipeline.intent_id.to_string()),
                Some(pipeline.action_type.as_str()),
                action_data.as_deref(),
                pipeline_json.as_deref(),
            )
            .await;
        recorder.append_tianhun_reason(tick_id, attempt, note).await;
    }
}

#[cfg(test)]
mod tests {
    use super::contains_meta_game_term;

    #[test]
    fn test_meta_term_detection_matches_reflector_ban_list() {
        // 元术语必须拦截（"温九辞被记忆标成 NPC"回归）
        assert!(contains_meta_game_term("关键NPC交互：温九辞递来一壶酒"));
        assert!(contains_meta_game_term("我的HP只剩4点"));
        assert!(contains_meta_game_term("这个玩家很友善"));
        assert!(contains_meta_game_term("等级提升了，经验值大增"));

        // 正常武侠叙事不得误伤
        assert!(!contains_meta_game_term("与温九辞对饮，谈笑甚欢"));
        assert!(!contains_meta_game_term("客栈中小憩，恢复体力"));
        assert!(!contains_meta_game_term("act as 一名镖师押送货物"));
        // ASCII 词仅大写精确匹配：英文普通词含 mp/san 不误伤
        assert!(!contains_meta_game_term("扎营过夜 camp，sample 货物"));
        // ASCII 词为大小写敏感子串匹配：小写 hp/npc 是当前设计的绕过口，
        // 用测试固化该取舍（防 camp/sample 误伤优先）
        assert!(!contains_meta_game_term("hp 只剩4点"));
        assert!(!contains_meta_game_term("这个 npc 很友善"));
    }
}
