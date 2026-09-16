//! 调度主循环（run）与 tick 广播/时间换算

use super::*;
use chrono::FixedOffset;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

impl TickScheduler {
    /// 启动Tick循环
    ///
    /// 实时模式：纯时钟驱动。
    /// 每个周期：广播 WorldState → 发送 TickBoundary（触发 IntentWorker 衰减）。
    /// Intent 不再由 scheduler 处理。
    pub async fn run(&mut self) -> Result<()> {
        let tick_duration_secs = {
            let gd = self.game_data_cache.get();
            gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64
        };

        info!("Tick引擎启动（实时模式），周期: {}秒", tick_duration_secs);
        info!("天道无为，万物自化。世界开始运转。");

        self.is_running = true;

        let game_epoch = self.parse_game_epoch()?;

        let db_max_tick_id = crate::db::get_current_world_tick_id(&self.db_pool)
            .await
            .unwrap_or(0);

        let time_based_tick_id = self.calculate_tick_id_from_time(game_epoch);
        self.current_tick_id = db_max_tick_id.max(time_based_tick_id);

        info!(
            "游戏纪元: {}, DB最大Tick: {}, 时间Tick: {}, 起始Tick: {}",
            game_epoch, db_max_tick_id, time_based_tick_id, self.current_tick_id
        );

        let mut interval = tokio::time::interval(Duration::from_secs(tick_duration_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        while self.is_running {
            // 热重载 actions.yaml
            if let Err(e) = self.check_and_reload_actions().await {
                warn!("动作热重载检查失败: {}", e);
            }

            // 热重载 game_rules.yaml
            if let Err(e) = self.check_and_reload_game_rules().await {
                warn!("游戏规则热重载检查失败: {}", e);
            }

            // 热重载 world_building_rules.yaml
            if let Err(e) = self.check_and_reload_world_building_rules().await {
                warn!("世界观规则热重载检查失败: {}", e);
            }

            // 热重载 prompt_templates.yaml
            if let Err(e) = self.check_and_reload_prompt_templates().await {
                warn!("Prompt 模板热重载检查失败: {}", e);
            }

            // 热重载 skills/
            if let Err(e) = self.check_and_reload_skills().await {
                warn!("技能热重载检查失败: {}", e);
            }

            // 热重载 narrative_config.yaml
            if let Err(e) = self.check_and_reload_narrative_config().await {
                warn!("叙事化配置热重载检查失败: {}", e);
            }

            interval.tick().await;

            self.tick_counter += 1;

            let new_tick_id = self.calculate_tick_id_from_time(game_epoch);
            self.current_tick_id = self.current_tick_id.max(new_tick_id);

            // 更新 accepting_tick_id（Agent 可用来判断当前 tick）
            self.accepting_tick_id
                .store(self.current_tick_id, Ordering::Release);

            // 1. 发送 TickBoundary 到 IntentWorker（触发衰减 + 死亡处理）
            if let Err(e) = self
                .worker_tx
                .send(WorkerMessage::TickBoundary {
                    tick_id: self.current_tick_id,
                })
                .await
            {
                error!(
                    "Tick {} 发送 TickBoundary 失败: {}",
                    self.current_tick_id, e
                );
            }

            // 1.5 Vendor 自动补货（在广播前执行，事件注入到 event_manager）
            if let Err(e) = self.refill_vendors(self.current_tick_id).await {
                warn!("Vendor 补货失败: {}", e);
            }

            // 2. 广播 WorldState
            if let Err(e) = self.broadcast_new_tick(self.current_tick_id).await {
                error!("Tick {} 广播失败: {}", self.current_tick_id, e);
            }

            // 2.5 游戏日边界推送：每个游戏日结束时向所有在线 Agent 推送动作统计
            // 使用 tick_counter（ordinal counter）而非 current_tick_id（墙钟秒），
            // 解除 modulo 对齐对墙钟余数的偶发依赖。
            let ticks_per_game_day = crate::game_data::registry::TimeRegistry::get_config()
                .map(|c| c.ticks_per_hour as u64 * c.hours_per_day as u64)
                .unwrap_or(12);
            if self.tick_counter > 0 && self.tick_counter.is_multiple_of(ticks_per_game_day) {
                let real_seconds_per_tick = {
                    let gd = self.game_data_cache.get();
                    gd.game_rules.data.agent_state.tick.real_seconds_per_tick as i64
                };
                let ticks_per_day_real_secs = ticks_per_game_day as i64 * real_seconds_per_tick;
                let game_day = self.current_tick_id / ticks_per_day_real_secs;
                let day_start_tick = self.current_tick_id - ticks_per_day_real_secs + 1;
                tracing::info!(
                    "[reward] 边界条件触发: tick_counter={}, current_tick_id={}, ticks_per_game_day={}, game_day={}",
                    self.tick_counter,
                    self.current_tick_id,
                    ticks_per_game_day,
                    game_day
                );
                // 生存 Reward 每日结算（旁路，失败只 error 不阻断 tick）
                // 数据源：agent_state_cache（DashMap，与 broadcast 同源，消除时序竞态）
                if let Err(e) = crate::reward::settle_daily(
                    &self.db_pool,
                    &self.agent_state_cache,
                    game_day,
                    self.current_tick_id,
                    day_start_tick,
                )
                .await
                {
                    error!(
                        "[reward] 每日结算失败 (game_day={}, tick={}): {}",
                        game_day, self.current_tick_id, e
                    );
                }
            }

            // 3. 群像传记：每 period_ticks 真实秒 (默认 7 游戏日) 生成一次
            // 转换为 tick 计数：period_ticks 是墙钟秒，除以 real_seconds_per_tick 得 tick 周期
            let period_ticks = crate::chronicle::ChronicleConfig::default().period_ticks;
            let real_seconds_per_tick = {
                let gd = self.game_data_cache.get();
                gd.game_rules.data.agent_state.tick.real_seconds_per_tick as i64
            };
            debug_assert!(
                period_ticks % real_seconds_per_tick == 0,
                "period_ticks({}) must be divisible by real_seconds_per_tick({})",
                period_ticks,
                real_seconds_per_tick
            );
            let chronicle_period_ticks = (period_ticks / real_seconds_per_tick) as u64;
            if self.tick_counter > 0 && self.tick_counter.is_multiple_of(chronicle_period_ticks) {
                let period_start = self.current_tick_id - period_ticks + 1;
                let db_pool = self.db_pool.clone();
                let tick_id = self.current_tick_id;
                // 生存 Reward 周期聚合（旁路，失败只 error 不阻断 tick）
                let pp_start = period_start;
                if let Err(e) =
                    crate::reward::settle_periodic(&self.db_pool, pp_start, tick_id).await
                {
                    error!(
                        "[reward] 周期聚合失败 (period={}~{}): {}",
                        pp_start, tick_id, e
                    );
                }
                tokio::spawn(async move {
                    match crate::chronicle::generate_and_store(period_start, tick_id, &db_pool)
                        .await
                    {
                        Ok(chronicle) => {
                            info!(
                                "群像传记生成完成: {} (第{}-{}日, {}季)",
                                chronicle.chronicle_id,
                                chronicle.game_day_start,
                                chronicle.game_day_end,
                                chronicle.season
                            );
                        }
                        Err(e) => {
                            error!("群像传记生成失败: {}", e);
                        }
                    }
                });
            }
        }

        info!("Tick引擎已停止");
        Ok(())
    }

    /// 广播新 tick 的 WorldState（从 DashMap 读取最新状态）
    pub(super) async fn broadcast_new_tick(&mut self, tick_id: i64) -> Result<()> {
        let agent_states: Vec<crate::models::AgentState> = self
            .agent_state_cache
            .iter()
            .map(|r| r.value().clone())
            .collect();

        self.event_manager.lock().expect("lock poisoned").clear();

        // drain grant-items 跨请求缓冲事件（clear 后注入，确保本 tick 可见）
        for entry in self.vendor_pending_events.iter() {
            let agent_id = *entry.key();
            for event in entry.value() {
                self.event_manager
                    .lock()
                    .expect("lock poisoned")
                    .add_event_for_agent(agent_id, event.clone());
            }
        }
        self.vendor_pending_events.clear();

        self.broadcaster
            .broadcast_states(
                tick_id,
                &agent_states,
                &self.db_pool,
                &self.connection_manager,
                &self.agent_to_device_map,
                &self.event_manager,
                &self.game_data_cache,
            )
            .await
            .context("广播: 广播状态失败")?;

        info!("Tick {} 广播完成: {}个Agent", tick_id, agent_states.len(),);
        Ok(())
    }

    /// 根据真实时间计算 tick ID（秒级秒数）
    ///
    /// tick_id = 当前Unix时间戳 - 游戏纪元
    /// 直接使用秒级秒数，real_seconds_per_tick 只影响执行频率，不影响 tick_id
    fn calculate_tick_id_from_time(&self, game_epoch: i64) -> i64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        now - game_epoch
    }

    /// 解析游戏纪元（从 YAML 配置）
    ///
    /// 使用配置的时区偏移量计算游戏纪元。
    /// 例如：start_date: "2026-03-03", timezone_offset: 8
    /// 表示 UTC+8 时区 2026-03-03 00:00:00，对应 UTC 2026-03-02 16:00:00。
    fn parse_game_epoch(&self) -> Result<i64> {
        let gd = self.game_data_cache.get();
        let start_date_str = gd.game_rules.data.agent_state.game_time.start_date.clone();
        let timezone_offset = gd.game_rules.data.agent_state.game_time.timezone_offset;
        drop(gd);

        // 解析日期字符串 (YYYY-MM-DD 格式)
        let date = chrono::NaiveDate::parse_from_str(&start_date_str, "%Y-%m-%d")
            .with_context(|| format!("无法解析游戏纪元日期: {}", start_date_str))?;

        // 使用配置的时区偏移量
        // 例如 UTC+8 = 8 * 3600 = 28800 秒
        let offset_seconds = timezone_offset * 3600;
        let offset = FixedOffset::east_opt(offset_seconds)
            .with_context(|| format!("无效的时区偏移量: {}", timezone_offset))?;

        let datetime = date.and_hms_opt(0, 0, 0).expect("midnight is always valid");
        let datetime_with_tz = datetime
            .and_local_timezone(offset)
            .single()
            .with_context(|| format!("无法创建时区感知时间: {}", start_date_str))?;

        let timestamp = datetime_with_tz.timestamp();

        // 计算对应的 UTC 时间用于日志
        let utc_datetime = datetime_with_tz.naive_utc();
        let utc_offset_sign = if timezone_offset >= 0 { "+" } else { "" };

        info!(
            "游戏纪元: {} 00:00:00 UTC{}{} = {} UTC (Unix timestamp: {})",
            start_date_str,
            utc_offset_sign,
            timezone_offset,
            utc_datetime.format("%Y-%m-%d %H:%M:%S"),
            timestamp
        );
        Ok(timestamp)
    }
}

// ============================================================================
// 测试
// ============================================================================
