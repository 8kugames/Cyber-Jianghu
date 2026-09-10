use std::sync::Arc;
use tokio::sync::Notify;
use tracing::{info, warn};

use super::super::reconnect::save_character_config_to_fs;
use crate::component::delta_engine::Urgency;
use crate::component::immediate::{EventStore, ImmediateEventHandler};
use crate::component::memory::backend::MemoryBackend;
use rand::RngExt;

/// 空转跳过占位摘要风味文本（零 LLM，进入 NarrativeSummaryWindow 保持时间连续性）
const IDLE_FLAVOR_DAY: &str = "（空转：无显著变化，未执行认知循环）";
const IDLE_FLAVOR_NIGHT: &str = "夜色已深，原地安歇。（夜间空转：未执行认知循环）";

impl super::super::Agent {
    /// 判定给定游戏小时是否属于夜间时段
    fn is_night_hour(&self, hour: i32) -> bool {
        is_night_hour(&self.config.token_optimization.idle_skip, hour)
    }

    /// 空转跳过判定（v1 保守方案）。
    ///
    /// 豁免层级（优先级从高到低）：
    /// 1. 生存驱动/实体/事件等 Important+ 信号：任何时段都唤醒（含夜间）；
    /// 2. 活跃对话会话：唤醒；
    /// 3. 黎明唤醒：离开夜间时段的第一个 tick 无条件完整思考；
    /// 4. 跳过上限（任何时段兜底）：连续跳过 max_consecutive_skips 后强制思考，夜间同样受约束；
    /// 5. 夜间：Info 空转跳过（无 whim；上限兜底仍适用）；
    /// 6. 白天：全 Info 空转按 1/whim_wake_divisor 概率照常思考（保留自发性）。
    ///
    /// 返回 true 表示跳过（streak +1）；false 表示照常思考（streak 清零）。
    pub(super) async fn should_skip_idle_tick(
        &mut self,
        world_state: &cyber_jianghu_protocol::WorldState,
    ) -> bool {
        let (idle_enabled, max_streak, whim_divisor) = {
            let cfg = &self.config.token_optimization.idle_skip;
            (
                cfg.enabled,
                cfg.max_consecutive_skips,
                cfg.whim_wake_divisor,
            )
        };

        let is_night = self.is_night_hour(world_state.world_time.hour);
        let is_dawn = self.idle_was_night && !is_night;
        self.idle_was_night = is_night;

        let dialogue_active = if let Some(ref dm) = self.dialogue_manager {
            dm.read().await.active_session_count() > 0
        } else {
            false
        };

        // 白天 whim 唤醒采样：真随机 1/N（独立采样天然分散，无羊群同步；
        // 不用 tick_id 做 hash——tick_id 为墙钟秒、按 tick 偶数步进，奇偶粘滞会使小除数退化）。
        // 采样提前于状态机：夜间/上限路径不消费该结果，仅多一次无副作用的随机数消耗。
        let whim_wake = whim_divisor <= 1 || rand::rng().random_bool(1.0 / whim_divisor as f64);

        let prev_streak = self.idle_skip_streak;
        let (skip, new_streak) = idle_skip_decision(
            idle_enabled,
            max_streak,
            whim_wake,
            is_night,
            is_dawn,
            self.idle_tick_candidate,
            dialogue_active,
            prev_streak,
        );
        self.idle_skip_streak = new_streak;

        // 日志语义与原分支一一对应：黎明/上限各自仅在对应分支真正触发时打印
        if !skip && idle_enabled && self.idle_tick_candidate && !dialogue_active {
            if is_dawn {
                info!(
                    "黎明唤醒: tick={}，离开夜间时段，执行完整认知循环",
                    world_state.tick_id
                );
            } else if prev_streak >= max_streak {
                info!(
                    "空转跳过连续 {} 个 tick 达到上限，本 tick 强制执行认知循环",
                    max_streak
                );
            }
        }

        skip
    }

    /// 空转 tick 记录：仅本地占位摘要 + 日志，零 LLM 消耗。
    /// 夜间用夜宿风味文本，保持叙事窗口的世界感与时间连续性。
    pub(super) async fn record_idle_tick(&self, world_state: &cyber_jianghu_protocol::WorldState) {
        let flavor = if self.is_night_hour(world_state.world_time.hour) {
            IDLE_FLAVOR_NIGHT
        } else {
            IDLE_FLAVOR_DAY
        };
        tracing::debug!(
            "空转跳过: tick={}, streak={}",
            world_state.tick_id,
            self.idle_skip_streak
        );
        if let Some(ref engine) = self.cognitive_engine {
            engine.record_idle_summary(world_state.tick_id, flavor);
        }
    }

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

            // 输出 game_day 计算值 + handler.current_game_day() 共享值
            tracing::debug!(
                "[WI-002-diagnose] main-loop tick={} game_day={} handler_current_game_day={}",
                world_state.tick_id,
                game_day,
                *handler.current_game_day().read().await
            );

            let need_spawn = match self.session_triage_handle {
                None => true,
                Some(ref handle) => handle.is_finished(),
            };
            if need_spawn {
                let prev_game_day = self.session_triage_game_day.take();
                self.session_triage_game_day = Some(game_day);

                // 输出 prev_game_day vs game_day 对比 + need_spawn 决策
                tracing::debug!(
                    "[WI-002-diagnose] session_triage spawn prev_game_day={:?} new_game_day={} will_cross={}",
                    prev_game_day,
                    game_day,
                    prev_game_day.is_some_and(|p| p != game_day)
                );

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
                                            let snapshot_id =
                                                world_state.agent_id.unwrap_or_default();
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

        let mut idle_candidate = false;
        let focus_summary = if self.config.token_optimization.enabled {
            if let (Some(store), Some(delta_engine), Some(attention_ctrl)) = (
                &self.world_state_store,
                &self.delta_engine,
                &self.attention_controller,
            ) {
                let prev = store.previous().await;
                let delta = delta_engine.compute(prev.as_ref(), world_state);
                // 空转跳过判定：非首 tick、本 tick 无 events_log 事件、且全部变化均为 Info 级。
                // 事件门控：server 每 tick 清空重注 events_log（相邻 tick 为独立事件集），
                // DeltaEngine 按长度比较可能漏检事件数持平的新事件——events_log 非空时一律思考，
                // 保证跳过 tick 不丢弃任何事件（v1 不制造记忆盲区）。
                // 唤醒保证：drive 支撑的生存属性变化至少 Important、实体出现/位置变化为 Important，
                // 这些信号天然打破全 Info 前提；无 drive 的亚阈值衰减为 Info，由跳过上限兜底。
                idle_candidate = !delta.is_first_tick
                    && world_state.events_log.is_empty()
                    && delta.changes.iter().all(|c| c.urgency == Urgency::Info);
                Some(attention_ctrl.filter(&delta))
            } else {
                None
            }
        } else {
            None
        };
        self.idle_tick_candidate = idle_candidate;
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

/// 判定游戏小时是否属于夜间时段（自由函数便于单测；hour 为 i32 对齐 WorldTime.hour）
fn is_night_hour(cfg: &crate::config::IdleSkipConfig, hour: i32) -> bool {
    cfg.night.enabled && cfg.night.night_hours.contains(&hour)
}

/// 空转跳过状态机核心（纯函数，便于单测；分支顺序即豁免层级）
///
/// 返回 (是否跳过, 新 streak)。`whim_wake` 为白天 whim 采样结果（调用方负责：
/// whim_wake_divisor<=1 时恒为 true；否则以 1/N 概率为 true）。
#[allow(clippy::too_many_arguments)] // 状态机输入信号天然为 8 个，结构体化反增样板（先例：broadcaster build_world_state_for_agent）
fn idle_skip_decision(
    enabled: bool,
    max_streak: usize,
    whim_wake: bool,
    is_night: bool,
    is_dawn: bool,
    candidate: bool,
    dialogue_active: bool,
    streak: usize,
) -> (bool, usize) {
    // 1. 总开关关闭 / Important+ 信号（candidate=false，任何时段唤醒）/ 对话活跃 / 黎明：照常思考
    if !enabled || !candidate || dialogue_active || is_dawn {
        return (false, 0);
    }
    // 2. 跳过上限（任何时段兜底，防长眠；也兼兑 night_hours 误配为全天）
    if streak >= max_streak {
        return (false, 0);
    }
    // 3. 夜间：Info 空转跳过（无 whim）；白天：whim 未唤醒则跳过
    if is_night || !whim_wake {
        return (true, streak + 1);
    }
    // 4. 白天 whim 唤醒：照常思考，streak 清零
    (false, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_night_hours_containment() {
        let cfg = crate::config::IdleSkipConfig::default();
        assert!(cfg.night.enabled);
        // 默认夜：9,10,11,0,1,2（跨零点）
        for h in [9i32, 10, 11, 0, 1, 2] {
            assert!(is_night_hour(&cfg, h), "hour {h} 应为夜间");
        }
        for h in [3i32, 5, 8] {
            assert!(!is_night_hour(&cfg, h), "hour {h} 应为白天");
        }
    }

    #[test]
    fn test_night_disabled() {
        let mut cfg = crate::config::IdleSkipConfig::default();
        cfg.night.enabled = false;
        assert!(!is_night_hour(&cfg, 10));
    }

    #[test]
    fn test_state_machine_disabled_always_thinks() {
        assert_eq!(
            idle_skip_decision(false, 4, false, true, false, true, false, 3),
            (false, 0)
        );
    }

    #[test]
    fn test_state_machine_important_signal_wakes_any_time() {
        // candidate=false：Important+ 信号（含夜间）打破全 Info 前提
        assert_eq!(
            idle_skip_decision(true, 4, false, true, false, false, false, 2),
            (false, 0)
        );
    }

    #[test]
    fn test_state_machine_dialogue_blocks_skip() {
        assert_eq!(
            idle_skip_decision(true, 4, false, true, false, true, true, 0),
            (false, 0)
        );
    }

    #[test]
    fn test_state_machine_dawn_wakes_and_resets_streak() {
        assert_eq!(
            idle_skip_decision(true, 4, false, false, true, true, false, 3),
            (false, 0)
        );
    }

    #[test]
    fn test_state_machine_cap_forces_think_any_time() {
        // 白天上限
        assert_eq!(
            idle_skip_decision(true, 4, false, false, false, true, false, 4),
            (false, 0)
        );
        // 夜间同样受上限约束（兜底 night_hours 误配全天）
        assert_eq!(
            idle_skip_decision(true, 4, false, true, false, true, false, 4),
            (false, 0)
        );
    }

    #[test]
    fn test_state_machine_night_skips_info_regardless_of_whim() {
        assert_eq!(
            idle_skip_decision(true, 4, true, true, false, true, false, 2),
            (true, 3)
        );
    }

    #[test]
    fn test_state_machine_day_whim_gate() {
        // whim 未唤醒 → 跳过
        assert_eq!(
            idle_skip_decision(true, 4, false, false, false, true, false, 1),
            (true, 2)
        );
        // whim 唤醒 → 照常思考，streak 清零
        assert_eq!(
            idle_skip_decision(true, 4, true, false, false, true, false, 1),
            (false, 0)
        );
    }
}
