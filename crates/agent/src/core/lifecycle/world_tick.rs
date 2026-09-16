// ============================================================================
// WorldState tick 主路径（handle_world_state_tick）
// ============================================================================
//
// 自 lifecycle/mod.rs 的 run select! 臂体提取（约 700 行单方法）。

use super::*;

impl crate::core::Agent {
    /// WorldState 臂主体：事件合并 → 死亡检查 → 空转跳过 → 记忆上下文 → 三魂循环 → 汇报。
    ///
    /// 从 `run` 的 select! 臂体提取；返回 `Continue` 表示主循环直接进入下一轮。
    #[allow(clippy::too_many_lines)]
    pub(super) async fn handle_world_state_tick(
        &mut self,
        agent_id: uuid::Uuid,
        world_state: cyber_jianghu_protocol::WorldState,
        death_rx: &mut Option<
            tokio::sync::broadcast::Receiver<cyber_jianghu_protocol::ServerMessage>,
        >,
        last_intents_for_narrative: &Arc<std::sync::Mutex<Vec<crate::models::Intent>>>,
    ) -> Result<ArmOutcome> {
        // 事件流合并：watch 只保最新快照，认知循环（LLM 推理秒级）期间
        // 到达的后续 WorldState 会覆盖本快照，其 events_log（含不可重复的
        // 死亡/攻击等事件）从事件队列 drain 回来，去重后并入当轮处理，
        // 进入记忆与特质演化通道（否则目击事件永久丢失）
        let pending = self.client.try_drain_pending_events().await;
        let world_state = if pending.is_empty() {
            world_state
        } else {
            let mut merged = world_state.clone();
            merged.events_log = merge_events_log(std::mem::take(&mut merged.events_log), pending);
            merged
        };

        self.update_tick_state(&world_state).await;

        // 1.5 检查是否死亡（只报告一次）
        // 路径 1: WorldState.events_log 中包含「自身」的 DeathNotification。
        //   目击他人死亡同样以 DeathNotification 形式投递（WitnessedDeath），
        //   必须比对死者 ID 与自身——否则目击者被误判为自身死亡且无自愈路径
        //   （见 death::find_self_death）
        // 路径 2: AgentDied WS 回调已设置 is_dead=true，但 events_log 可能已过期
        let self_death = if self.death_reported {
            None
        } else {
            death::find_self_death(&world_state.events_log, world_state.agent_id)
        };
        let death_via_callback = !self.death_reported
            && self
                .http_api_state
                .as_ref()
                .map(|s| s.is_dead.load(std::sync::atomic::Ordering::Relaxed))
                .unwrap_or(false);

        if self_death.is_some() || death_via_callback {
            let death_desc = self_death
                .map(|e| e.description.as_str())
                .unwrap_or("AgentDied 回调通知（events_log 未包含 DeathNotification）");
            self.handle_death(
                world_state.tick_id,
                world_state.agent_id.unwrap_or_default(),
                death_desc,
            )
            .await;
            return Ok(ArmOutcome::Continue);
        }

        // 1.5b 已死亡 → 跳过决策循环（等待重生恢复）
        if self.death_reported {
            // 双源检查：优先 WS 回调值（动态），fallback 到 config 值（注册时下发的 game_rules）
            let effective_delay_ticks = if self.rebirth_delay_ticks > 0 {
                self.rebirth_delay_ticks
            } else {
                self.config.rebirth_delay_ticks()
            };
            if effective_delay_ticks > 0 {
                let rebirth_done = self
                    .http_api_state
                    .as_ref()
                    .map(|s| !s.is_dead.load(std::sync::atomic::Ordering::Relaxed))
                    .unwrap_or(false);
                if rebirth_done {
                    info!(
                        "Agent '{}' 自动重生恢复决策: tick={}",
                        self.character_name(),
                        world_state.tick_id
                    );
                    self.death_reported = false;
                    self.death_tick_id = None;
                    if let Some(ref mut char_cfg) = self.character_config {
                        char_cfg.status = crate::config::CharacterStatus::Alive;
                        if let Some(ref api_state) = self.http_api_state {
                            let characters_dir = api_state.character_dir.read().await.clone();
                            if let Err(e) = save_character_config_to_fs(char_cfg, &characters_dir) {
                                warn!("Failed to persist rebirth status: {}", e);
                            }
                        }
                    }
                } else {
                    return Ok(ArmOutcome::Continue);
                }
            } else {
                // 无自动重生（WS 回调 + config 均为 0）：持续等待直到外部触发（通过 API 或重启）
                return Ok(ArmOutcome::Continue);
            }
        }

        // 空转跳过：delta 无显著变化且本 tick 无 events_log 事件时不执行认知循环
        // （token_optimization.idle_skip，默认开启）。
        // v1 保守节律：夜间抑制 Info 空转 + 黎明唤醒 + 白天 whim 1/N（默认 1/2）；
        // 插入点位于死亡检查之后、记忆上下文构建之前。
        if self.should_skip_idle_tick(&world_state).await {
            self.record_idle_tick(&world_state).await;
            return Ok(ArmOutcome::Continue);
        }

        // 构建记忆上下文（事件消费 + 遗忘 + 对话 + 交易提示 + triage）
        let (mut memory_context, trade_hints) = self.build_tick_memory_context(&world_state).await;

        // 4.5 天魂执行叙事生成（上一轮行动结果，用于 memory_context 和 soul_cycle_record 回填）
        {
            let last_intents = last_intents_for_narrative
                .lock()
                .expect("lock poisoned")
                .clone();

            // 数据驱动的上轮行动摘要：从 soul_cycle_recorder 读取上轮人魂叙事
            let last_action_summary = if !last_intents.is_empty() {
                if let Some(recorder) = self.soul_recorder().await {
                    match recorder
                        .get_last_renhun_narrative(world_state.tick_id)
                        .await
                    {
                        Ok(Some(narrative)) => Some(format!(
                            "【重要】你上一轮的行动：{}。不要进行无谓的重复。",
                            narrative
                        )),
                        Ok(None) => None,
                        Err(e) => {
                            warn!("get_last_renhun_narrative 失败（best-effort 跳过）: {e:?}");
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };

            // 上一轮行动结果注入 memory_context
            if let Some(ref summary) = last_action_summary {
                memory_context.push_str(&format!("\n### 上一轮行动结果\n{}\n", summary));
            }
        }

        // 4.3 交易议价提示注入
        if !trade_hints.is_empty() {
            memory_context.push_str("\n### 交易提示\n");
            memory_context.push_str(&trade_hints.join("\n"));
        }

        // 4.4 托梦注入（统一路径：消费 dream 并注入 memory_context）
        // 注意：注入源保持原文（不影响 agent 推理），脱敏在 trace record 时做
        let active_dream: Option<String> = if let Some(ref api_state) = self.http_api_state
            && let Some(dream_thought) = api_state.consume_dream().await
        {
            info!(
                "[dream] 托梦注入决策上下文: {}字",
                dream_thought.chars().count()
            );
            memory_context.push_str("\n### 托梦\n");
            memory_context.push_str(&dream_thought);
            memory_context.push('\n');
            Some(dream_thought)
        } else {
            None
        };

        // 4.4b 跨 Agent 死亡知识传播：不注入任何服务器聚合的"教训/传言"。
        // 传播链路为纯涌现：目击者轻出死亡事件（death_notification，
        // 已进记忆与特质演化）→ 目击者自主选择"说"（speak 动作）→
        // 同位置者听见 → 随行走扩散。天道无为，不替众生传话。

        // 4.5 决策上下文快照写入（供 /api/v1/context enrichment 使用）
        if let Some(ref api_state) = self.http_api_state {
            let (summary_ctx, outcome_ctx, action_desc, action_hints) =
                if let Some(ref engine) = self.cognitive_engine {
                    let (desc, hints) = engine.get_action_context();
                    (
                        engine.get_summary_context(),
                        engine.get_outcome_context_public(),
                        desc,
                        hints,
                    )
                } else {
                    (String::new(), String::new(), String::new(), String::new())
                };

            // 读取上次执行结果（如果有）
            let last_exec = api_state
                .decision_context_snapshot
                .read()
                .await
                .as_ref()
                .and_then(|s| s.last_execution_result.clone());

            let snapshot = crate::infra::api::DecisionContextSnapshot {
                tick_id: world_state.tick_id,
                memory_context: memory_context.clone(),
                summary_context: summary_ctx,
                outcome_section: outcome_ctx,
                action_descriptions: action_desc,
                action_field_hints: action_hints,
                last_execution_result: last_exec,
            };
            *api_state.decision_context_snapshot.write().await = Some(snapshot);
        }

        // 三魂循环（ActorSoul → ReflectorSoul 审查 + 后置处理）
        //
        // 中断防护：认知循环内含 LLM 重试管线（game_rules.intent_batch
        // .max_retries 次退避重试），flaky LLM 下可运行数十分钟；期间主
        // select! 无法轮询死亡/转世/重连通道。2026-09-15 柳青崖事故：
        // 死亡到达时决策正处于重试中 → 死后持续空烧 LLM 且错过
        // rebirth_notify（Notify 只唤醒当前等待者），仅重启容器可恢复。
        // 此处与三类信号竞速，任一到达即放弃本轮决策（in-flight future
        // 被 drop），回到主循环由对应分支正式处理（AgentDied →
        // handle_death → maybe_schedule_auto_rebirth 自动转世链）。
        // 接收端用 resubscribe/subscribe 独立订阅，不与主分支争用广播。
        let interrupt_api_state = self.http_api_state.clone();
        let mut interrupt_death_rx = death_rx.as_ref().map(|rx| rx.resubscribe());
        let mut interrupt_reconnect_rx = interrupt_api_state
            .as_ref()
            .and_then(|s| s.reconnect_tx.as_ref().map(|tx| tx.subscribe()));
        let soul_result = tokio::select! {
            r = self.run_three_soul_cycle(
                &world_state,
                &memory_context,
                active_dream.as_deref(),
                last_intents_for_narrative,
            ) => r?,
            _ = async {
                match interrupt_death_rx.as_mut() {
                    Some(rx) => { let _ = rx.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                info!("[main] tick={} 认知循环期间收到死亡通知，中断本轮决策", world_state.tick_id);
                return Ok(ArmOutcome::Continue);
            }
            _ = async {
                match interrupt_reconnect_rx.as_mut() {
                    Some(rx) => { let _ = rx.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                info!("[main] tick={} 认知循环期间收到重连请求，中断本轮决策", world_state.tick_id);
                return Ok(ArmOutcome::Continue);
            }
            _ = async {
                match interrupt_api_state.as_ref() {
                    Some(s) => s.rebirth_notify.notified().await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                info!("[main] tick={} 认知循环期间收到转世通知，中断本轮决策", world_state.tick_id);
                return Ok(ArmOutcome::Continue);
            }
        };
        let mut final_intent = soul_result.intent;
        let final_intent_validated = soul_result.validated;
        let soul_cycle_attempt = soul_result.attempt;

        let graded_config = self
            .config
            .game_rules
            .as_ref()
            .and_then(|g| g.intent_batch.as_ref())
            .map(|b| b.llm_validation.clone());

        // 6. 发送意图
        if !final_intent_validated {
            match self
                .validate_with_reflector(
                    final_intent.clone(),
                    &world_state,
                    graded_config.as_ref(),
                    Vec::new(),
                )
                .await?
            {
                crate::soul::reflector::PipelineValidationResult::Approved {
                    intent: approved,
                    ..
                } => {
                    final_intent = approved;
                }
                crate::soul::reflector::PipelineValidationResult::Rejected { reason, .. } => {
                    self.set_rejection_feedback(reason.clone(), world_state.tick_id);
                    warn!(
                        "Tick {} fallback intent 被天魂驳回，改用 chaos fallback: {}",
                        world_state.tick_id, reason
                    );
                    final_intent = self.chaos_fallback_intent(
                        &world_state,
                        agent_id,
                        format!("fallback 被天魂驳回: {}", reason),
                    );
                    // 留痕：实际发送的是二次 chaos 意图，覆写 final intent
                    // 并在天魂理由说明替换缘由（此前零记录）
                    self.record_chaos_override(
                        world_state.tick_id,
                        soul_cycle_attempt,
                        &final_intent,
                        &format!(
                            "fallback 被天魂驳回，已替换为 chaos: {}（驳回原因: {}）",
                            final_intent.action_type.as_str(),
                            reason
                        ),
                    )
                    .await;
                }
            }
        }

        // 三魂元数据随 intent 一次性提交（消除独立 SoulCycleReport 的丢失风险）
        let soul_cycle_metadata = self.build_soul_cycle_metadata(final_intent.tick_id).await;
        if let Err(e) = self
            .client
            .send_intent(&final_intent, soul_cycle_metadata)
            .await
        {
            error!("Failed to send intent: {}", e);
            if let Err(reconnect_err) = self.reconnect().await {
                error!("Reconnect failed: {}", reconnect_err);
            }
        } else {
            info!(
                "Intent sent successfully: tick={}, action={}, agent={}",
                final_intent.tick_id, final_intent.action_type, final_intent.agent_id
            );

            // 记录 Agent 自身对话消息（原子化：write lock 内解析 session_id）
            {
                use crate::component::dialogue::DialogueRole;
                if let Some(ref dm) = self.dialogue_manager {
                    let action_type_str = final_intent.action_type.as_str();
                    let is_dialogue = {
                        let dm = dm.read().await;
                        dm.is_dialogue_action(action_type_str)
                    };
                    if is_dialogue {
                        let content = final_intent
                            .action_data
                            .as_ref()
                            .and_then(|d| d.get("content"))
                            .and_then(|c| c.as_str())
                            .unwrap_or("");
                        if !content.is_empty() {
                            let target_id = final_intent
                                .action_data
                                .as_ref()
                                .and_then(|d| d.get("target_agent_id"))
                                .and_then(|t| t.as_str())
                                .and_then(|s| Uuid::parse_str(s).ok());
                            let tick = self.current_tick.load(std::sync::atomic::Ordering::Relaxed);
                            let mut guard = dm.write().await;
                            let session_id = if let Some(tid) = target_id {
                                guard
                                    .get_session_id_by_partner(&tid)
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| {
                                        format!(
                                            "{}{}",
                                            crate::component::dialogue::PENDING_SESSION_PREFIX,
                                            tid
                                        )
                                    })
                            } else {
                                format!("speak_{}", chrono::Utc::now().timestamp())
                            };
                            guard.add_message(
                                &session_id,
                                final_intent.agent_id,
                                DialogueRole::Own,
                                content,
                                tick,
                            );
                        }
                    }
                }
            }

            // 实时模式：等待 ExecutionResult（server 立即处理后的反馈）
            // Pipeline 语义：失败只阻断后续 intent，前序成功 intent 已生效。
            // 使用 mpsc channel 收集全部多 intent 结果
            match self
                .client
                .wait_for_execution_result(self.config.llm.execution_result_timeout_ms)
                .await
            {
                Ok(results) if !results.is_empty() => {
                    let success_count = results.iter().filter(|r| r.success).count();
                    let total = results.len();
                    let all_success = success_count == total;
                    let first_failure = results.iter().find(|r| !r.success);

                    debug!(
                        "ExecutionResult: tick={}, {}/{} success",
                        results[0].tick_id, success_count, total
                    );

                    // 执行结果回填 SoulCycleRecord
                    if let Some(recorder) = self.soul_recorder().await {
                        use std::collections::HashMap;
                        let tick_id = results[0].tick_id;
                        if let Ok(records) = recorder.get_by_tick(tick_id).await {
                            let mut attempt_results: HashMap<
                                (i64, i32),
                                serde_json::Map<String, serde_json::Value>,
                            > = HashMap::new();
                            for result in &results {
                                let rid = result.intent_id.to_string();
                                for record in &records {
                                    let mut found_pipe_seq: Option<usize> = None;
                                    // 检查主 Intent 匹配 (pipe_seq=0)
                                    if let Some(ref fid) = record.final_intent_id
                                        && fid == &rid
                                    {
                                        found_pipe_seq = Some(0);
                                    }
                                    // 检查 pipeline 子 Intent 匹配 (final_pipeline_json 现在含 intent_id)
                                    if found_pipe_seq.is_none()
                                        && let Some(ref json) = record.final_pipeline_json
                                        && let Ok(pipeline) =
                                            serde_json::from_str::<Vec<serde_json::Value>>(json)
                                    {
                                        for (i, entry) in pipeline.iter().enumerate() {
                                            if entry.get("intent_id").and_then(|v| v.as_str())
                                                == Some(&rid)
                                            {
                                                found_pipe_seq = Some(i);
                                                break;
                                            }
                                        }
                                    }
                                    if let Some(pipe_seq) = found_pipe_seq {
                                        let exec_map = attempt_results
                                            .entry((tick_id, record.attempt))
                                            .or_default();
                                        exec_map.insert(
                                            pipe_seq.to_string(),
                                            serde_json::json!({
                                                "success": result.success,
                                                "error": result.error,
                                                "state_change_summary": result.state_change_summary,
                                            }),
                                        );
                                        break;
                                    }
                                }
                            }
                            for ((tid, att), exec_map) in &attempt_results {
                                let json = serde_json::to_string(exec_map).unwrap_or_default();
                                recorder.backfill_server_result(*tid, *att, &json).await;
                            }
                        }
                    }

                    // 建立 intent_id → (action_type, action_data) 映射表
                    let intent_map: std::collections::HashMap<
                        uuid::Uuid,
                        (
                            &cyber_jianghu_protocol::ActionType,
                            &Option<serde_json::Value>,
                        ),
                    > = {
                        let mut map = std::collections::HashMap::new();
                        map.insert(
                            final_intent.intent_id,
                            (&final_intent.action_type, &final_intent.action_data),
                        );
                        for si in &final_intent.subsequent_intents {
                            map.insert(si.intent_id, (&si.action_type, &si.action_data));
                        }
                        map
                    };

                    // intent 失败且 agent 已死亡 → 立即触发死亡处理
                    // 双路径检测：(1) is_dead 原子标志 (2) server error 含 "is dead"/"not in cache"
                    if !self.death_reported && first_failure.is_some() {
                        let is_dead_now = self
                            .http_api_state
                            .as_ref()
                            .map(|s| s.is_dead.load(std::sync::atomic::Ordering::Relaxed))
                            .unwrap_or(false);
                        // Path 4: server error 明确告知 agent 已死亡
                        // （WebSocket AgentDied 消息可能未到达，is_dead 未设置）
                        let error_str =
                            first_failure.and_then(|r| r.error.as_deref()).unwrap_or("");
                        let server_says_dead =
                            error_str.contains("is dead") || error_str.contains("not in cache");
                        if is_dead_now || server_says_dead {
                            let reason_str =
                                first_failure.and_then(|r| r.error.as_deref()).unwrap_or("");
                            warn!(
                                "Agent '{}' 检测到死亡（intent 失败后）: {}",
                                self.character_name(),
                                reason_str
                            );
                            self.death_reported = true;
                            self.death_tick_id = Some(world_state.tick_id);

                            if let Some(ref api_state) = self.http_api_state {
                                self.rebirth_delay_ticks = api_state
                                    .rebirth_delay_ticks
                                    .load(std::sync::atomic::Ordering::Relaxed);
                            }

                            if let Some(ref mut char_cfg) = self.character_config {
                                char_cfg.status = crate::config::CharacterStatus::Dead;
                                if let Some(ref api_state) = self.http_api_state {
                                    let characters_dir =
                                        api_state.character_dir.read().await.clone();
                                    if let Err(e) =
                                        save_character_config_to_fs(char_cfg, &characters_dir)
                                    {
                                        warn!("Failed to persist death status: {}", e);
                                    }
                                }
                            }

                            death::maybe_schedule_auto_rebirth(
                                self,
                                world_state.agent_id.unwrap_or_default(),
                                world_state.tick_id,
                                "（intent失败路径）",
                            )
                            .await;
                        }
                    }

                    // v6 简化：失败 intent 触发治理提案，跳过 SelfEvaluator
                    // IR 由 Server 端从 proposed_action_type 查表生成
                    for result in &results {
                        if !result.success {
                            let governance_code = result.governance_code.unwrap_or(
                                cyber_jianghu_protocol::GovernanceCode::NonGovernanceReject,
                            );

                            // NonGovernanceReject 不提交（普通参数错误）
                            if matches!(
                                governance_code,
                                cyber_jianghu_protocol::GovernanceCode::NonGovernanceReject
                            ) {
                                return Ok(ArmOutcome::Continue);
                            }

                            let (action_type_str, action_data) = intent_map
                                .get(&result.intent_id)
                                .map(|(at, ad)| (at.to_string(), (*ad).clone()))
                                .unwrap_or_else(|| ("unknown".to_string(), None));

                            let proposal = serde_json::json!({
                                "agent_id": agent_id,
                                "tick_id": result.tick_id,
                                "proposed_action_type": action_type_str,
                                "action_data": action_data,
                                "rationale": result.error.clone().unwrap_or_default(),
                            });
                            let url = format!(
                                "{}/api/v1/action-evolution/propose",
                                self.config.server.http_url
                            );
                            let auth_token = self
                                .device_config
                                .as_ref()
                                .map(|d| d.auth_token.clone())
                                .unwrap_or_default();
                            tokio::spawn(async move {
                                let _ = reqwest::Client::new()
                                    .post(&url)
                                    .header(
                                        reqwest::header::AUTHORIZATION,
                                        format!("Bearer {}", auth_token),
                                    )
                                    .json(&proposal)
                                    .send()
                                    .await;
                            });
                        }
                    }

                    // Outcome 写回：逐条记录每个 intent 的执行结果
                    let context_hash = crate::component::memory::compute_context_hash(&world_state);
                    for result in &results {
                        let (action_type, action_data) = intent_map
                            .get(&result.intent_id)
                            .map(|(at, ad)| (at.to_string(), (*ad).clone()))
                            .unwrap_or_else(|| ("unknown".to_string(), None));

                        if let Some(ref engine) = self.cognitive_engine {
                            let target_agent_id =
                                crate::component::memory::extract_target_agent_id(&action_data);
                            engine.record_outcome(crate::component::memory::OutcomeRecord {
                                action_type: action_type.clone(),
                                action_data: action_data.clone(),
                                result: if result.success {
                                    crate::component::memory::OutcomeResult::Success
                                } else {
                                    crate::component::memory::OutcomeResult::Failed(
                                        result.error.clone().unwrap_or_default(),
                                    )
                                },
                                target_agent_id,
                                context_hash: context_hash.clone(),
                                tick_id: result.tick_id,
                            });
                        }
                    }

                    // 上轮行动结果因果链：逐条格式化注入下轮人魂推理
                    if let Some(ref engine) = self.cognitive_engine {
                        let result_ids: std::collections::HashSet<uuid::Uuid> =
                            results.iter().map(|r| r.intent_id).collect();
                        let mut lines: Vec<String> = Vec::new();

                        // 按 intent_map 原始顺序输出（primary + subsequent）
                        // primary intent
                        if let Some((at, ad)) = intent_map.get(&final_intent.intent_id)
                            && let Some(result) = results
                                .iter()
                                .find(|r| r.intent_id == final_intent.intent_id)
                        {
                            let desc = Self::summarize_intent(
                                at.as_str(),
                                ad.as_ref(),
                                &world_state.location.name,
                                &world_state.entities,
                            );
                            if result.success {
                                lines.push(format!("- {} → 成功", desc));
                            } else {
                                let reason = result.error.as_deref().unwrap_or("未知原因");
                                lines.push(format!("- {} → 失败（{}）", desc, reason));
                            }
                        }
                        // subsequent intents（保持 pipeline 原始顺序）
                        for si in &final_intent.subsequent_intents {
                            if let Some(result) =
                                results.iter().find(|r| r.intent_id == si.intent_id)
                            {
                                let desc = Self::summarize_intent(
                                    si.action_type.as_str(),
                                    si.action_data.as_ref(),
                                    &world_state.location.name,
                                    &world_state.entities,
                                );
                                if result.success {
                                    lines.push(format!("- {} → 成功", desc));
                                } else {
                                    let reason = result.error.as_deref().unwrap_or("未知原因");
                                    lines.push(format!("- {} → 失败（{}）", desc, reason));
                                }
                            } else if !result_ids.contains(&si.intent_id) {
                                // Saga rollback: 前序失败导致此 intent 未执行
                                let desc = Self::summarize_intent(
                                    si.action_type.as_str(),
                                    si.action_data.as_ref(),
                                    &world_state.location.name,
                                    &world_state.entities,
                                );
                                lines.push(format!("- {} → 未执行（因前序动作失败被跳过）", desc));
                            }
                        }
                        engine.set_last_tick_action_summary(lines.join("\n"));
                    }

                    // Summary 更新
                    if let Some(ref engine) = self.cognitive_engine {
                        let label = if all_success {
                            format!("成功: {}", final_intent.action_type)
                        } else {
                            let failed_action = first_failure
                                .and_then(|r| intent_map.get(&r.intent_id))
                                .map(|(at, _)| at.to_string())
                                .unwrap_or_default();
                            format!(
                                "部分成功 ({}/{}): {} | 失败: {}",
                                success_count, total, final_intent.action_type, failed_action
                            )
                        };
                        engine.update_summary_outcome(label);
                    }

                    // 失败部分：注入失败原因到下轮推理上下文
                    if let Some(failed) = first_failure {
                        let reason = failed.error.clone().unwrap_or_default();
                        let failed_action = intent_map
                            .get(&failed.intent_id)
                            .map(|(at, _)| at.to_string())
                            .unwrap_or_default();
                        {
                            let mut guard = self.server_error_feedback.lock().await;
                            *guard = Some(format!(
                                "[pipeline 部分失败 ({}/{}): {} 失败原因: {}]",
                                success_count, total, failed_action, reason
                            ));
                        }
                    }

                    // 更新执行结果到快照
                    if let Some(ref api_state) = self.http_api_state {
                        let mut snapshot = api_state.decision_context_snapshot.write().await;
                        if let Some(s) = snapshot.as_mut() {
                            let failed_action = first_failure
                                .and_then(|r| intent_map.get(&r.intent_id))
                                .map(|(at, _)| at.to_string());
                            s.last_execution_result = Some(crate::infra::api::ExecutionSummary {
                                action_type: final_intent.action_type.to_string(),
                                success: all_success,
                                narrative: if all_success {
                                    format!("{} intents all success", total)
                                } else {
                                    format!(
                                        "{}/{} success, {} 失败: {}",
                                        success_count,
                                        total,
                                        failed_action.as_deref().unwrap_or("unknown"),
                                        first_failure
                                            .and_then(|r| r.error.clone())
                                            .unwrap_or_default()
                                    )
                                },
                            });
                        }
                    }
                }
                Ok(_) => {
                    debug!(
                        "ExecutionResult timeout ({}ms), no results received",
                        self.config.llm.execution_result_timeout_ms
                    );
                    // 超时无结果时清除旧摘要，防止下轮显示过期数据
                    if let Some(ref engine) = self.cognitive_engine {
                        engine.set_last_tick_action_summary(String::new());
                    }
                }
                Err(e) => {
                    debug!("ExecutionResult poll error: {}", e);
                    if let Some(ref engine) = self.cognitive_engine {
                        engine.set_last_tick_action_summary(String::new());
                    }
                }
            }

            if final_intent.action_type.as_str() != "休整" {
                self.consecutive_idle_count = 0;
                if let Some(ref container) = self.actor_llm_container {
                    let llm = container.read().await;
                    llm.reset_idle_count();
                }
            }
            if final_intent.action_type.as_str() == "休整" {
                self.maybe_rotate_model().await;
            }

            self.report_soul_cycle_and_compress(&final_intent).await;
        }
        Ok(ArmOutcome::Handled)
    }
}
