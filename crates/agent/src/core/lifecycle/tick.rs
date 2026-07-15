use std::sync::Arc;
use tokio::sync::Notify;
use tracing::{info, warn};

use super::super::reconnect::save_character_config_to_fs;
use crate::component::immediate::{EventStore, ImmediateEventHandler};
use crate::component::memory::backend::MemoryBackend;

impl super::super::Agent {
    pub(super) async fn update_tick_state(
        &mut self,
        world_state: &cyber_jianghu_protocol::WorldState,
    ) {
        self.current_tick
            .store(world_state.tick_id, std::sync::atomic::Ordering::Relaxed);
        if let Some(ref dm) = self.dialogue_manager {
            let mut guard = dm.write().await;
            guard.cleanup_timed_out(world_state.tick_id);
        }
        // 延迟初始化: game_rules 在 build 之后才从 Server 到达
        if self.immediate_handler.is_none() {
            self.try_init_immediate_handler().await;
        }

        if let Some(ref handler) = self.immediate_handler {
            handler.set_tick_id(world_state.tick_id).await;
            let game_day = Self::compute_game_day(
                &world_state.world_time,
                self.config
                    .game_rules
                    .as_ref()
                    .and_then(|g| g.calendar.as_ref()),
            );
            handler.set_game_day(game_day).await;

            let need_spawn = match self.session_triage_handle {
                None => true,
                Some(ref handle) => handle.is_finished(),
            };
            if need_spawn {
                let prev_game_day = self.session_triage_game_day.take();
                self.session_triage_game_day = Some(game_day);
                if let Some(old_handle) = self.session_triage_handle.take() {
                    match old_handle.await {
                        Ok(summary_opt) => {
                            if let Some(ref summary) = summary_opt {
                                let summary_game_day = prev_game_day.unwrap_or(game_day);
                                if let Some(ref mm) = self.memory_manager {
                                    let importance = self
                                        .config
                                        .game_rules
                                        .as_ref()
                                        .and_then(|g| g.immediate_events.as_ref())
                                        .and_then(|ie| ie.event_triage.as_ref())
                                        .map(|et| et.daily_summary_importance as f32)
                                        .unwrap_or(0.8);
                                    let mut entry = crate::component::memory::MemoryEntry::new(
                                        world_state.agent_id.unwrap_or_default(),
                                        world_state.tick_id,
                                        summary.clone(),
                                    )
                                    .with_event_type("daily_summary".to_string())
                                    .with_importance(importance);
                                    let mut mm_guard = mm.write().await;
                                    match mm_guard.episodic_mut().add(&mut entry).await {
                                        Ok(_) => {
                                            info!(
                                                "游戏日 {} 摘要已存储到 episodic memory (importance={:.1})",
                                                summary_game_day, importance
                                            );
                                        }
                                        Err(e) => {
                                            warn!("游戏日摘要写入 episodic memory 失败: {}", e);
                                        }
                                    }
                                }

                                let ds_config = self
                                    .config
                                    .game_rules
                                    .as_ref()
                                    .and_then(|g| g.daily_summary.as_ref());
                                let max_retries = ds_config.map(|c| c.max_retries).unwrap_or(3);
                                let base_delay_ms = ds_config
                                    .map(|c| (c.ttl_ticks as u64).min(1000))
                                    .unwrap_or(100);

                                let mut submitted = false;
                                for attempt in 0..max_retries {
                                    match self
                                        .client
                                        .send_daily_summary(summary_game_day, summary)
                                        .await
                                    {
                                        Ok(()) => {
                                            info!(
                                                "游戏日 {} 摘要已提交 Server (attempt {})",
                                                summary_game_day,
                                                attempt + 1
                                            );
                                            submitted = true;
                                            break;
                                        }
                                        Err(e) => {
                                            warn!(
                                                "游戏日 {} 摘要提交 Server 失败 (attempt {}/{}): {}",
                                                summary_game_day,
                                                attempt + 1,
                                                max_retries,
                                                e
                                            );
                                            if attempt + 1 < max_retries {
                                                let delay = base_delay_ms * (1 << attempt);
                                                tokio::time::sleep(
                                                    tokio::time::Duration::from_millis(delay),
                                                )
                                                .await;
                                            }
                                        }
                                    }
                                }
                                if !submitted {
                                    warn!(
                                        "游戏日 {} 摘要提交 Server 最终失败（已重试 {} 次）",
                                        summary_game_day, max_retries
                                    );
                                }

                                // C1: 关系图谱全量快照同步
                                // 游戏日结束时随 DailySummary 一起上报，server 全量覆盖（DELETE+INSERT）。
                                // 时间戳转换：agent 本地 DateTime<Utc> → protocol i64 毫秒。
                                if let Some(ref store) = self.relationship_store {
                                    match store.get_all_relationships() {
                                        Ok(local_rels) => {
                                            let snapshot_id = world_state.agent_id
                                                .unwrap_or_default();
                                            if snapshot_id.is_nil() {
                                                warn!(
                                                    "关系快照跳过：agent_id 未知（game_day={}）",
                                                    summary_game_day
                                                );
                                            } else {
                                                let proto_rels: Vec<
                                                    cyber_jianghu_protocol::types::RelationshipMemory,
                                                > = local_rels
                                                    .iter()
                                                    .map(|r| {
                                                        cyber_jianghu_protocol::types::RelationshipMemory {
                                                            target_agent_id: r.target_agent_id,
                                                            target_name: r.target_name.clone(),
                                                            favorability: r.favorability,
                                                            key_events: r
                                                                .key_events
                                                                .iter()
                                                                .map(|e| {
                                                                    cyber_jianghu_protocol::types::RelationshipKeyEvent {
                                                                        tick_id: e.tick_id,
                                                                        event_type: e.event_type.clone(),
                                                                        description: e.description.clone(),
                                                                        favorability_delta: e.favorability_delta,
                                                                        timestamp: e.timestamp.timestamp_millis(),
                                                                    }
                                                                })
                                                                .collect(),
                                                            last_interaction_tick: r.last_interaction_tick,
                                                            updated_at: r.updated_at.timestamp_millis(),
                                                            self_description: r.self_description.clone(),
                                                            description_tick: r.description_tick,
                                                        }
                                                    })
                                                    .collect();

                                                let count = proto_rels.len();
                                                match self
                                                    .client
                                                    .send_relationship_snapshot(
                                                        snapshot_id,
                                                        summary_game_day,
                                                        proto_rels,
                                                    )
                                                    .await
                                                {
                                                    Ok(()) => {
                                                        info!(
                                                            "游戏日 {} 关系快照已提交 Server (count={})",
                                                            summary_game_day, count
                                                        );
                                                    }
                                                    Err(e) => {
                                                        warn!(
                                                            "游戏日 {} 关系快照提交 Server 失败: {}",
                                                            summary_game_day, e
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!(
                                                "游戏日 {} 读取本地关系存储失败，跳过快照: {}",
                                                summary_game_day, e
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            if e.is_panic() {
                                warn!("SessionTriageEngine panic（将被重启）: {}", e);
                            } else {
                                warn!("SessionTriageEngine 被取消: {}", e);
                            }
                        }
                    }
                }
                if let Some(ref llm_container) = self.actor_llm_container {
                    let triage_config = handler.event_store().config().clone();

                    let diary_prompt = self.cognitive_engine.as_ref().and_then(|engine| {
                        let config = engine.prompt_template();
                        let tmpl = config.get_template("daily_diary")?;
                        tmpl.sections.get("system").cloned()
                    });

                    let engine = crate::component::immediate::SessionTriageEngine::new(
                        handler.event_store().clone(),
                        llm_container.clone(),
                        self.extract_persona(),
                        self.character_name().to_string(),
                        triage_config,
                        game_day,
                        handler.current_game_day(),
                        Some(world_state.world_time.clone()),
                        self.memory_manager.clone(),
                        self.relationship_store.clone(),
                        self.world_state_store.clone(),
                        diary_prompt,
                    );
                    self.session_triage_handle = Some(tokio::spawn(engine.run()));
                    info!(
                        "SessionTriageEngine 已 spawn: agent={}, game_day={}",
                        self.character_name(),
                        game_day
                    );
                }
            }
        }

        if let Some(ref store) = self.world_state_store {
            store.update(world_state.clone()).await;
        }

        let focus_summary = if self.config.token_optimization.enabled {
            if let (Some(store), Some(delta_engine), Some(attention_ctrl)) = (
                &self.world_state_store,
                &self.delta_engine,
                &self.attention_controller,
            ) {
                let prev = store.previous().await;
                let delta = delta_engine.compute(prev.as_ref(), world_state);
                let summary = attention_ctrl.filter(&delta);
                Some(summary)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(ref summary) = focus_summary {
            *self.current_focus_summary.write().await = Some(summary.clone());
            if let Some(ref engine) = self.cognitive_engine {
                engine
                    .set_current_focus_summary(Some(summary.clone()))
                    .await;
            }
        } else {
            if let Some(ref engine) = self.cognitive_engine {
                engine.set_current_focus_summary(None).await;
            }
        }
        if let Some(ref api_state) = self.http_api_state {
            let mut current = api_state.current_state.write().await;
            *current = Some(world_state.clone());

            let mut last_update = api_state.last_state_update.write().await;
            *last_update = Some(std::time::Instant::now());

            api_state.maybe_update_narratives(world_state).await;
        }

        if let Some(ref mut char_cfg) = self.character_config {
            char_cfg.last_connected_real_time = Some(chrono::Utc::now());
            char_cfg.last_connected_world_time = Some(world_state.world_time.clone());

            if let Some(ref api_state) = self.http_api_state {
                let char_cfg_clone = char_cfg.clone();
                let characters_dir = api_state.character_dir.read().await.clone();
                tokio::spawn(async move {
                    if let Err(e) = save_character_config_to_fs(&char_cfg_clone, &characters_dir) {
                        warn!("Failed to save character last_connected time: {}", e);
                    }
                });
            }
        }

        // persona trait 衰减 + 缓存刷新（让下一 tick LLM 看到最新状态）
        self.persona.write(|p| p.apply_all_decay());
        if let Some(ref engine) = self.cognitive_engine {
            engine.invalidate_persona_cache(&self.persona);
        }

        if let Some(ref store) = self.persona_store {
            let interval = store.config_snapshot_interval();
            if interval > 0
                && world_state.tick_id % interval == 0
                && let Err(e) = self
                    .persona
                    .read(|p| store.snapshot(p, world_state.tick_id))
            {
                warn!("persona 周期快照失败: {}", e);
            }
        }
    }

    /// 延迟初始化 ImmediateEventHandler（game_rules 配置到达后创建）
    async fn try_init_immediate_handler(&mut self) {
        let game_rules = match self.client.game_rules().await {
            Some(gr) => gr,
            None => return,
        };
        let immediate_events = match game_rules.immediate_events {
            Some(ref ie) => ie,
            None => return,
        };
        let triage_config = match immediate_events.event_triage {
            Some(ref cfg) => cfg,
            None => return,
        };
        if triage_config.pre_filter.fallback_thresholds().is_err() {
            warn!("event_triage.pre_filter 阈值无效，跳过延迟初始化");
            return;
        }
        let notify = Arc::new(Notify::new());
        match EventStore::open(&self.data_dir, triage_config, notify) {
            Ok(store) => {
                let handler = Arc::new(ImmediateEventHandler::new(Arc::new(store)));
                self.set_immediate_handler(handler);
            }
            Err(e) => {
                warn!("EventStore 延迟初始化失败: {}", e);
            }
        }
    }
}
