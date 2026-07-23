// ============================================================================
// Server 回调设置
// ============================================================================

use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::component::memory::backend::MemoryBackend;

impl super::super::Agent {
    /// 设置客户端级回调（game_rules/skill/prompt_template/dialogue/world_building_rules）
    pub(super) async fn setup_client_callbacks(&mut self) {
        let agent_name_for_callback = self.character_name().to_string();
        let agent_name_for_skills = agent_name_for_callback.clone();
        let agent_name_for_prompt = agent_name_for_callback.clone();

        let cognitive_engine_for_rules = self.cognitive_engine.clone();
        self.client
            .set_game_rules_callback(Arc::new(move |game_rules| {
                info!(
                    "Agent '{}' received game rules update: version {}",
                    agent_name_for_callback, game_rules.version
                );
                if let Some(ref engine) = cognitive_engine_for_rules {
                    engine.update_action_index(&game_rules.available_actions);
                    engine.set_available_actions(game_rules.available_actions.clone());
                }
            }))
            .await;

        let cognitive_engine_for_skills = self.cognitive_engine.clone();
        self.client
            .set_skill_update_callback(Arc::new(move |skills, removed_items| {
                info!(
                    "Agent '{}' received skill config update: {} skills, {} removed",
                    agent_name_for_skills,
                    skills.len(),
                    removed_items.len()
                );
                if let Some(ref engine) = cognitive_engine_for_skills {
                    engine.update_skill_cache(skills, removed_items);
                }
            }))
            .await;

        let cognitive_engine_for_prompt = self.cognitive_engine.clone();
        let validator_for_prompt = self.validator.clone();
        self.client
            .set_prompt_template_callback(Arc::new(
                move |config: cyber_jianghu_protocol::PromptTemplateConfig| {
                    info!(
                        "Agent '{}' received prompt_templates config update: version={}",
                        agent_name_for_prompt, config.version
                    );
                    if let Some(ref engine) = cognitive_engine_for_prompt {
                        engine.update_prompt_template_from_config(config.clone());
                        engine.save_prompt_template_to_disk();
                    }
                    if let Some(ref validator) = validator_for_prompt {
                        validator.update_prompt_config(std::sync::Arc::new(config));
                    }
                },
            ))
            .await;

        if self.dialogue_client.is_some() {
            let dialogue_client = self.dialogue_client.clone();
            let agent_name_for_dialogue = self.character_name().to_string();
            self.client
                .set_dialogue_callback(Arc::new(move |message| {
                    debug!(
                        "Agent '{}' received dialogue message",
                        agent_name_for_dialogue
                    );
                    if let Some(ref dc) = dialogue_client {
                        dc.handle_message(message);
                    }
                }))
                .await;
            info!(
                "Dialogue callback set for agent '{}'",
                self.character_name()
            );
        }

        let agent_name_for_event_rules = self.character_name().to_string();
        let event_trait_mapper_for_rules = self.event_trait_mapper.clone();
        self.client
            .set_persona_event_rules_callback(Arc::new(
                move |rules: Vec<crate::component::persona::TraitMappingRule>| {
                    info!(
                        "Agent '{}' received persona_event_rules update: {} rules",
                        agent_name_for_event_rules,
                        rules.len()
                    );
                    let new_mapper = crate::component::persona::EventTraitMapper::from_rules(rules);
                    if let Ok(mut guard) = event_trait_mapper_for_rules.write() {
                        *guard = new_mapper;
                        debug!("EventTraitMapper updated from Server ConfigUpdate");
                    } else {
                        warn!("EventTraitMapper RwLock poisoned — cannot update rules");
                    }
                },
            ))
            .await;

        if self.validator.is_some() {
            let validator = self.validator.clone();
            let agent_name_for_rules = self.character_name().to_string();
            self.client
                .set_world_building_rules_callback(Arc::new(move |rules| {
                    info!(
                        "Agent '{}' received world building rules update: version {}",
                        agent_name_for_rules, rules.version
                    );
                    if let Some(ref v) = validator {
                        let v_clone = v.clone();
                        let rules_clone = rules.clone();
                        tokio::spawn(async move {
                            v_clone.update_rules(rules_clone).await;
                        });
                    }
                }))
                .await;
            info!(
                "World building rules callback set for agent '{}'",
                self.character_name()
            );
        }

        // narrative_config 热更新回调
        let api_state_for_nc = self.http_api_state.clone();
        self.client
            .set_narrative_config_callback(Arc::new(
                move |nc: cyber_jianghu_protocol::NarrativeConfig, hash: Option<String>| {
                    info!(
                        "Received narrative_config ConfigUpdate: version={}",
                        nc.version
                    );
                    if let Some(ref api_state) = api_state_for_nc {
                        // 用 tokio::spawn 避免在同步回调中 await
                        let nc = nc.clone();
                        let hash = hash.clone();
                        let api_state = api_state.clone();
                        tokio::spawn(async move {
                            *api_state.narrative_config.write().await = Some(nc.clone());
                            if let Err(e) =
                                crate::config::save_narrative_config_to_disk(&nc, hash.as_deref())
                            {
                                warn!("热更新保存 narrative_config 失败: {}", e);
                            }
                        });
                    }
                },
            ))
            .await;
    }

    /// 构建并设置 Server 消息回调（链式：lifecycle 处理 + binary 回调透传）
    pub(super) async fn build_and_set_server_message_callback(&mut self) {
        let prev_callback = self.client.get_server_msg_callback().await;
        let api_state = self.http_api_state.clone();
        let immediate_handler = self.immediate_handler.clone();
        let error_feedback = self.server_error_feedback.clone();
        let memory_manager = self.memory_manager.clone();
        let dialogue_manager = self.dialogue_manager.clone();
        let game_rules = self.config.game_rules.clone();
        let current_tick = self.current_tick.clone();
        let callback: Arc<dyn Fn(cyber_jianghu_protocol::ServerMessage) + Send + Sync> =
            Arc::new(move |msg: cyber_jianghu_protocol::ServerMessage| {
                if let cyber_jianghu_protocol::ServerMessage::Error { code, message, .. } = &msg
                    && code == cyber_jianghu_protocol::ERROR_CODE_ACTION_FAILED
                {
                    let reason = message.clone();
                    let feedback = error_feedback.clone();
                    tokio::spawn(async move {
                        let mut guard = feedback.lock().await;
                        *guard = Some(reason);
                    });
                }
                if let cyber_jianghu_protocol::ServerMessage::ImmediateEvent { .. } = &msg
                    && let Some(ref handler) = immediate_handler
                {
                    let h = handler.clone();
                    let msg = msg.clone();
                    tokio::spawn(async move {
                        h.handle_server_message(msg).await;
                    });
                }
                if let cyber_jianghu_protocol::ServerMessage::Dialogue { message } = &msg {
                    use crate::component::dialogue::DialogueRole;
                    use cyber_jianghu_protocol::DialogueMessage;

                    let dm = dialogue_manager.clone();
                    let dialogue_message = message.clone();
                    let tick = current_tick.load(std::sync::atomic::Ordering::Relaxed);

                    tokio::spawn(async move {
                        let Some(ref dm) = dm else {
                            return;
                        };
                        let mut guard = dm.write().await;

                        match dialogue_message {
                            DialogueMessage::Content {
                                session_id,
                                from_agent_id,
                                content,
                            } => {
                                guard.add_message(
                                    &session_id,
                                    from_agent_id,
                                    DialogueRole::Partner,
                                    &content,
                                    tick,
                                );
                            }
                            DialogueMessage::Request {
                                from_agent_id,
                                opening_remark,
                                ..
                            } => {
                                let session_id = format!(
                                    "request_{}_{}",
                                    from_agent_id,
                                    chrono::Utc::now().timestamp()
                                );
                                guard.add_message(
                                    &session_id,
                                    from_agent_id,
                                    DialogueRole::Partner,
                                    &opening_remark,
                                    tick,
                                );
                            }
                            DialogueMessage::Accept {
                                session_id,
                                from_agent_id,
                            } => {
                                let pending_id = format!(
                                    "{}{}",
                                    crate::component::dialogue::PENDING_SESSION_PREFIX,
                                    from_agent_id
                                );
                                guard.migrate_session(
                                    &pending_id,
                                    &session_id,
                                    from_agent_id,
                                    tick,
                                );
                                guard.add_message(
                                    &session_id,
                                    from_agent_id,
                                    DialogueRole::Partner,
                                    "[对方接受了对话请求]",
                                    tick,
                                );
                            }
                            DialogueMessage::Reject {
                                session_id,
                                from_agent_id,
                                reason,
                            } => {
                                let pending_id = format!(
                                    "{}{}",
                                    crate::component::dialogue::PENDING_SESSION_PREFIX,
                                    from_agent_id
                                );
                                guard.close_session(&pending_id);
                                warn!(
                                    "Dialogue rejected by {}: session={}, reason={:?}",
                                    from_agent_id, session_id, reason
                                );
                            }
                            DialogueMessage::End { session_id, .. } => {
                                guard.end_session(&session_id);
                            }
                        }
                    });
                }
                if let cyber_jianghu_protocol::ServerMessage::DailySummaryData {
                    game_day,
                    action_counts,
                    location_history,
                    success_count,
                    failure_count,
                    total_actions,
                } = &msg
                {
                    let mm = memory_manager.clone();
                    let gr = game_rules.clone();
                    let gd = *game_day;
                    let ac = action_counts.clone();
                    let lh = location_history.clone();
                    let sc = *success_count;
                    let fc = *failure_count;
                    let ta = *total_actions;
                    tokio::spawn(async move {
                        if let Some(ref mgr) = mm {
                            let importance = gr
                                .as_ref()
                                .and_then(|g| g.immediate_events.as_ref())
                                .and_then(|ie| ie.event_triage.as_ref())
                                .map(|et| et.daily_summary_importance as f32)
                                .unwrap_or(0.8);
                            let mut sorted: Vec<_> = ac.iter().collect();
                            sorted.sort_by(|a, b| b.1.cmp(a.1));
                            let action_parts: Vec<String> = sorted
                                .iter()
                                .take(5)
                                .map(|(k, v)| format!("{}x{}", k, v))
                                .collect();
                            let content = format!(
                                "第{}游戏日动作统计：共{}次（成{}、败{}）。动作：{}{}",
                                gd,
                                ta,
                                sc,
                                fc,
                                action_parts.join("、"),
                                if lh.is_empty() {
                                    String::new()
                                } else {
                                    format!("；足迹：{}", lh.join("→"))
                                }
                            );
                            let entry_agent_id = {
                                let r = mgr.read().await;
                                r.agent_id()
                            };
                            let mut entry = crate::component::memory::MemoryEntry::new(
                                entry_agent_id,
                                gd,
                                content,
                            )
                            .with_event_type("daily_action_stats".to_string())
                            .with_importance(importance);
                            let mut guard = mgr.write().await;
                            if let Err(e) = guard.episodic_mut().add(&mut entry).await {
                                warn!("游戏日{}动作统计写入 episodic memory 失败: {}", gd, e);
                            }
                        }
                    });
                }
                // 死亡事件：设置死亡标记并广播到 death_event_tx
                // Cognitive 模式下 AgentDied 回调已做此操作，此路径确保 Claw 模式也能正确检测死亡
                if let cyber_jianghu_protocol::ServerMessage::AgentDied {
                    rebirth_delay_ticks,
                    ..
                } = &msg
                    && let Some(ref s) = api_state
                {
                    s.is_dead.store(true, std::sync::atomic::Ordering::Relaxed);
                    s.rebirth_delay_ticks
                        .store(*rebirth_delay_ticks, std::sync::atomic::Ordering::Relaxed);
                    if let Err(e) = s.death_event_tx.send(msg.clone()) {
                        tracing::warn!("death_event_tx.send（callbacks）失败（receiver 可能已 drop）：{e:?}");
                    }
                }
                if let Some(ref prev) = prev_callback {
                    prev(msg);
                }
            });
        self.client.set_server_msg_callback(callback).await;
        info!("Server 消息回调已注册（即时事件 + 验证错误 + 链式透传）");
    }
}
