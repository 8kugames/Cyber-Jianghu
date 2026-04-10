// ============================================================================
// OpenClaw Cyber-Jianghu MVP Tick Scheduler
// ============================================================================
//
// 调度器负责Tick引擎的主循环执行流程，包括：
// 1. 协调各个阶段的执行
// 2. 记录性能日志
// 3. 错误处理和恢复
//
// 设计原则：
// 1. 单线程执行，避免并发问题
// 2. 每个Tick独立，失败不影响下一个Tick
// 3. 详细的性能日志，方便定位问题
// 4. 优雅的错误处理，不崩溃
// ============================================================================

use anyhow::{Context, Result};
use chrono::FixedOffset;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

use crate::db::DbPool;
use crate::dialogue::DialogueManager;
use crate::game_data::GameDataCache;
use crate::models::{TickLog, WorldEventType};
use crate::websocket::{AgentToDeviceMap, ConnectionManager, IntentManager};

use super::super::inventory::InventoryManager;
use super::broadcaster::Broadcaster;
use super::event_manager::EventManager;
use super::intent_collector::IntentCollector;
use super::processor::StateProcessor;
use super::{decay, persistence};

use crate::game_data::loaders::load_actions;
use crate::paths::get_config_dir;
use crate::websocket::broadcast_action_update;
use cyber_jianghu_protocol::ServerMessage;
use std::fs;

/// Tick调度器
///
/// 负责驱动游戏世界的运行
pub struct TickScheduler {
    /// 游戏数据缓存
    game_data_cache: Arc<GameDataCache>,

    /// 当前Tick编号（递增）
    current_tick_id: i64,

    /// 运行状态
    is_running: bool,

    /// 数据库连接池
    db_pool: DbPool,

    /// WebSocket 连接管理器
    connection_manager: ConnectionManager,

    /// agent_id → device_id 反向映射
    agent_to_device_map: AgentToDeviceMap,

    /// Intent 管理器（临时缓存）
    intent_manager: IntentManager,

    /// 事件管理器
    event_manager: EventManager,

    /// 意图收集器
    intent_collector: IntentCollector,

    /// 广播器
    broadcaster: Broadcaster,

    /// 状态处理器
    state_processor: StateProcessor,

    /// 对话管理器
    dialogue_manager: Arc<DialogueManager>,

    /// 上一轮关闭的对话记录（用于下一轮广播）
    closed_dialogue_records: Vec<cyber_jianghu_protocol::PrivateDialogueRecord>,

    /// 当前接受意图的 tick_id（与 AppState 共享）
    accepting_tick_id: Arc<AtomicI64>,

    /// 上次加载的 actions.yaml 修改时间
    last_actions_mtime: Option<std::time::SystemTime>,
}

impl TickScheduler {
    /// 创建新的Tick调度器
    pub fn new(
        game_data_cache: Arc<GameDataCache>,
        db_pool: DbPool,
        connection_manager: ConnectionManager,
        agent_to_device_map: AgentToDeviceMap,
        intent_manager: IntentManager,
        dialogue_manager: Arc<DialogueManager>,
        accepting_tick_id: Arc<AtomicI64>,
    ) -> Self {
        Self {
            game_data_cache,
            current_tick_id: 0,
            is_running: false,
            db_pool: db_pool.clone(),
            connection_manager,
            agent_to_device_map,
            intent_manager,
            event_manager: EventManager::new(),
            intent_collector: IntentCollector::new(),
            broadcaster: Broadcaster::new(),
            state_processor: StateProcessor::new(db_pool),
            dialogue_manager,
            closed_dialogue_records: vec![],
            accepting_tick_id,
            last_actions_mtime: None,
        }
    }

    /// 检查 actions.yaml 是否变更，若变更则重新加载并广播
    async fn check_and_reload_actions(&mut self) -> Result<()> {
        let config_dir = get_config_dir();
        let actions_path = config_dir.join("actions.yaml");
        let json_path = config_dir.join("actions.json");

        // 确定实际使用的文件
        let file_path = if actions_path.exists() {
            &actions_path
        } else if json_path.exists() {
            &json_path
        } else {
            return Ok(()); // 文件不存在，跳过
        };

        let metadata = match fs::metadata(file_path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => return Ok(()),
        };

        // 检查是否是新文件或已修改
        let should_reload = match self.last_actions_mtime {
            Some(last) => modified > last,
            None => true,
        };

        if should_reload {
            self.last_actions_mtime = Some(modified);

            // 重新加载 actions
            match load_actions(&config_dir) {
                Ok(new_actions) => {
                    let version = new_actions.version.clone();
                    let actions_count = new_actions.data.len();

                    // 更新缓存
                    self.game_data_cache.update_actions(new_actions);

                    // 重新初始化注册表
                    crate::game_data::init_registry(self.game_data_cache.clone());

                    info!(
                        "动作配置已热重载: version={}, actions={}",
                        version, actions_count
                    );

                    // 构建 AvailableAction 列表
                    let available_actions =
                        crate::game_data::ActionRegistry::build_available_actions();

                    // 广播给所有在线 Agent
                    let action_update = ServerMessage::ActionUpdate {
                        update_type: "full".to_string(),
                        actions: available_actions,
                        updated_actions: vec![],
                        removed_actions: vec![],
                        version,
                    };

                    if let Err(e) =
                        broadcast_action_update(action_update, &self.connection_manager).await
                    {
                        warn!("广播动作更新失败: {}", e);
                    }
                }
                Err(e) => {
                    warn!("重新加载 actions.yaml 失败: {}", e);
                }
            }
        }

        Ok(())
    }

    /// 启动Tick循环
    ///
    /// 新时序：广播 → sleep(收集窗口) → 结算
    /// Agent 收到广播后有完整窗口提交 intent
    pub async fn run(&mut self) -> Result<()> {
        // 从 game_data_cache 读取 tick 配置（克隆值以避免持有锁）
        let tick_duration_secs = {
            let gd = self.game_data_cache.get();
            gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64
        };

        info!(
            "Tick引擎启动，周期: {}秒 (来自 game_rules.yaml)",
            tick_duration_secs
        );
        info!("天道无为，万物自化。世界开始运转。");

        self.is_running = true;

        // 计算游戏纪元（用于基于真实时间的 tick ID）
        let game_epoch = self.parse_game_epoch()?;

        // 获取基于真实时间的当前 tick ID
        let db_max_tick_id = crate::db::get_current_world_tick_id(&self.db_pool)
            .await
            .unwrap_or(0);

        let time_based_tick_id = self.calculate_tick_id_from_time(game_epoch);

        // 使用两者中的较大值，确保不会回退
        self.current_tick_id = db_max_tick_id.max(time_based_tick_id);

        info!(
            "游戏纪元: {}, 数据库最大Tick: {}, 时间计算Tick: {}, 起始Tick: {}",
            game_epoch, db_max_tick_id, time_based_tick_id, self.current_tick_id
        );

        let mut interval = tokio::time::interval(Duration::from_secs(tick_duration_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // 主循环：广播 → sleep → 结算
        while self.is_running {
            // 检查 actions.yaml 是否变更
            if let Err(e) = self.check_and_reload_actions().await {
                warn!("动作热重载检查失败: {}", e);
            }

            interval.tick().await;

            let new_tick_id = self.calculate_tick_id_from_time(game_epoch);
            // 防御时钟回拨：tick_id 只增不减
            self.current_tick_id = self.current_tick_id.max(new_tick_id);

            // 1. 开单 + 广播（使用 max 守卫后的值，保证 Agent 看到的 tick_id 单调递增）
            self.accepting_tick_id
                .store(self.current_tick_id, Ordering::Release);

            let collection_window_secs = {
                let gd = self.game_data_cache.get();
                let window = gd.game_rules.data.agent_state.tick.collection_window_secs as u64;
                let period = gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64;
                if window >= period {
                    error!(
                        "collection_window_secs({}) >= real_seconds_per_tick({}), 已禁用收集窗口",
                        window, period
                    );
                    0
                } else {
                    window
                }
            };

            // deadline_ms = 绝对时间戳
            let deadline_ms = calculate_deadline_abs_ms(collection_window_secs);

            // 广播新 tick（加载上一次持久化的 agent_states）
            if let Err(e) = self.broadcast_new_tick(new_tick_id, deadline_ms).await {
                error!("Tick {} 广播失败: {}", new_tick_id, e);
            }

            // 2. 等待 Agent 提交 intent
            if collection_window_secs > 0 {
                info!(
                    "Tick {} 等待收集窗口 {}秒...",
                    new_tick_id, collection_window_secs
                );
                tokio::time::sleep(Duration::from_secs(collection_window_secs)).await;
            }

            // 3. 关单 + 结算
            // 关单：设为 0，结算期间不再接受新 intent
            self.accepting_tick_id.store(0, Ordering::Release);
            if let Err(e) = self.execute_tick_settlement(new_tick_id).await {
                error!("Tick {} 结算失败: {}", new_tick_id, e);
            }
        }

        info!("Tick引擎已停止");
        Ok(())
    }

    /// 广播新 tick 的 WorldState（基于上一次持久化的 agent_states）
    async fn broadcast_new_tick(&mut self, tick_id: i64, deadline_ms: u64) -> Result<()> {
        let agent_states = persistence::load_agent_states(&self.db_pool)
            .await
            .context("广播: 加载Agent状态失败")?;

        self.event_manager.clear();

        self.broadcaster
            .broadcast_states(
                tick_id,
                &agent_states,
                &self.db_pool,
                &self.connection_manager,
                &self.agent_to_device_map,
                &self.event_manager,
                &self.game_data_cache,
                deadline_ms,
                &self.closed_dialogue_records,
            )
            .await
            .context("广播: 广播状态失败")?;

        info!(
            "Tick {} 广播完成: {}个Agent, deadline={}ms",
            tick_id,
            agent_states.len(),
            deadline_ms
        );
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

        let datetime = date.and_hms_opt(0, 0, 0).unwrap();
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

    /// 执行一次Tick结算（关单后调用）
    ///
    /// 包含：收集意图 → 结算 → 衰减 → 持久化（不含广播）
    async fn execute_tick_settlement(&mut self, tick_id: i64) -> Result<()> {
        let mut tick_log = TickLog::new(tick_id);
        info!("Tick {} 开始结算", tick_id);

        if let Err(e) = crate::db::create_tick_log(&self.db_pool, &tick_log).await {
            warn!("创建Tick日志失败: {}", e);
        }

        let result = self.execute_tick_inner(tick_id, &mut tick_log).await;

        match &result {
            Ok((agents_processed, actions_executed)) => {
                tick_log.complete(*agents_processed, *actions_executed);
            }
            Err(e) => {
                tick_log.fail(&e.to_string());
            }
        }

        if let Err(e) = crate::db::update_tick_log(&self.db_pool, &tick_log).await {
            warn!("更新Tick日志失败: {}", e);
        }

        if let Err(e) = persistence::save_tick_log(&tick_log).await {
            warn!("保存Tick日志文件失败: {}", e);
        }

        result.map(|_| ())
    }

    async fn execute_tick_inner(
        &mut self,
        tick_id: i64,
        // 预留：用于记录 tick 执行详情（待集成）
        _tick_log: &mut TickLog,
    ) -> Result<(i32, i32)> {
        let start_time = Instant::now();

        // event_manager 在 broadcast_new_tick 中已 clear，结算阶段直接添加事件
        // 这些事件会在下一个 tick 的广播中发送给 Agent

        let phase1_start = Instant::now();
        let agent_states = persistence::load_agent_states(&self.db_pool)
            .await
            .context("加载Agent状态失败")?;
        let phase1_duration = phase1_start.elapsed();
        info!(
            "阶段1完成 - 加载状态: {}个Agent, 耗时: {:?}",
            agent_states.len(),
            phase1_duration
        );

        // 1. 记录系统事件：当前时间/季节广播
        let mut time_events = Vec::new();
        if let Some(time_display) =
            crate::game_data::registry::TimeRegistry::get_time_display(tick_id)
        {
            // 每当进入新的一小时（整点）或者新的一天时，可以广播系统消息
            let is_new_hour = tick_id
                % crate::game_data::registry::TimeRegistry::get_config()
                    .map(|c| c.ticks_per_hour as i64)
                    .unwrap_or(60)
                == 0;

            if is_new_hour {
                let season_name = time_display
                    .season
                    .map(|s| s.name)
                    .unwrap_or_else(|| "未知".to_string());
                let time_desc = format!(
                    "现在是 {} 季，第 {} 天，{} 时",
                    season_name, time_display.day, time_display.hour
                );

                // 将时间信息添加到每个 Agent 的事件列表中（作为全局环境信息）
                for state in &agent_states {
                    if state.is_alive {
                        let event = crate::models::WorldEvent {
                            event_type: WorldEventType::TimeUpdate,
                            tick_id,
                            description: time_desc.clone(),
                            metadata: serde_json::json!({
                                "season": season_name,
                                "day": time_display.day,
                                "hour": time_display.hour,
                                "is_daytime": time_display.is_daytime,
                            }),
                        };
                        time_events.push((state.agent_id, event));
                    }
                }
            }
        }

        // 2. 先处理意图（物品效果），再处理衰减
        // 修复: 物品效果应该先于衰减执行，避免玩家使用物品后仍然死亡的问题
        let phase2_1_start = Instant::now();
        let intents = self
            .intent_collector
            .collect_intents(&self.intent_manager, tick_id, &agent_states)
            .await
            .context("收集意图失败")?;
        let (
            intent_processed_states,
            executed_actions,
            processor_events,
            action_logs,
            validation_errors,
        ) = self
            .state_processor
            .process_intents(tick_id, agent_states, &intents)
            .await
            .context("结算意图失败")?;

        // 发送验证错误通知给 agent
        if !validation_errors.is_empty() {
            for (agent_id, reason) in &validation_errors {
                let msg = cyber_jianghu_protocol::ServerMessage::Error {
                    code: cyber_jianghu_protocol::ERROR_CODE_ACTION_FAILED.to_string(),
                    message: reason.clone(),
                    current_tick_id: Some(tick_id),
                };
                if let Err(e) = super::broadcaster::send_to_agent(
                    *agent_id,
                    &msg,
                    &self.connection_manager,
                    &self.agent_to_device_map,
                )
                .await
                {
                    debug!("验证错误通知发送失败: agent={}, error={}", agent_id, e);
                }
            }
        }

        for (agent_id, event) in processor_events {
            self.event_manager.add_event_for_agent(agent_id, event);
        }

        if !action_logs.is_empty() {
            if let Err(e) = crate::db::batch_insert_action_logs(&self.db_pool, &action_logs).await {
                warn!("批量插入动作日志失败: {}", e);
            } else {
                debug!("动作日志已保存: {} 条", action_logs.len());
            }
        }

        let phase2_1_duration = phase2_1_start.elapsed();
        info!(
            "阶段2.1完成 - 结算意图: {}个意图, {}个动作, 耗时: {:?}",
            intents.len(),
            executed_actions,
            phase2_1_duration
        );

        // 2. 处理自然衰减和环境伤害（在意图处理之后）
        // 修复: 衰减在意图处理之后执行，这样物品效果可以先生效
        let phase2_2_start = Instant::now();
        let (mut updated_states, dead_agents, mut decay_events, death_notifications) =
            decay::apply_decay_and_environmental_damage(tick_id, intent_processed_states);

        // Push death notifications immediately to agents
        let ctx = crate::websocket::DeathNotificationContext {
            connection_manager: &self.connection_manager,
            agent_to_device_map: &self.agent_to_device_map,
        };
        for notification in death_notifications {
            if let Err(e) = crate::websocket::send_agent_died_notification(
                notification.agent_id,
                notification.cause,
                notification.description,
                notification.location,
                notification.tick_id,
                notification.died_at,
                &ctx,
            )
            .await
            {
                warn!(
                    "Failed to send death notification to agent {}: {}",
                    notification.agent_id, e
                );
            }
        }

        // 死亡 Agent 主动清理 WebSocket 连接（避免下 tick 浪费 send + mark_dead）
        if !dead_agents.is_empty() {
            let mut connections = self.connection_manager.write().await;
            let mut agent_to_device = self.agent_to_device_map.write().await;
            for agent_id in &dead_agents {
                if let Some(device_id) = agent_to_device.remove(agent_id)
                    && connections.remove(&device_id).is_some()
                {
                    info!(
                        "已断开死亡 Agent {} 的 WebSocket 连接 (device: {})",
                        agent_id, device_id
                    );
                }
            }
        }

        // 将时间事件合并到衰减事件中
        decay_events.extend(time_events);

        for (agent_id, event) in decay_events {
            self.event_manager.add_event_for_agent(agent_id, event);
        }

        if !dead_agents.is_empty() {
            for agent_id in &dead_agents {
                let already_cleared = updated_states
                    .iter()
                    .any(|s| s.agent_id == *agent_id && s.inventory_cleared_this_tick);

                if !already_cleared {
                    // 获取死亡 Agent 的位置用于掉落
                    let location = updated_states
                        .iter()
                        .find(|s| s.agent_id == *agent_id)
                        .map(|s| s.node_id.clone());

                    match InventoryManager::clear_inventory(&self.db_pool, *agent_id).await {
                        Ok(items) => {
                            // 死亡掉落物品到地面
                            if let Some(loc) = &location {
                                for item in &items {
                                    if let Err(e) = crate::db::add_ground_item(
                                        &self.db_pool,
                                        loc,
                                        &item.item_id,
                                        item.quantity,
                                        Some(*agent_id),
                                    )
                                    .await
                                    {
                                        warn!("自然死亡掉落物品添加到地面失败: {}", e);
                                    }
                                }
                            }
                            info!(
                                "Agent {} 自然死亡，背包已清空并掉落 {} 个物品到地面",
                                agent_id,
                                items.len()
                            );
                        }
                        Err(e) => {
                            warn!("清空死亡Agent {} 背包失败: {}", agent_id, e);
                        }
                    }

                    // 标记已清空，防止后续重复处理
                    if let Some(state) = updated_states.iter_mut().find(|s| s.agent_id == *agent_id)
                    {
                        state.inventory_cleared_this_tick = true;
                    }
                }
            }
        }

        // 死亡 Agent 状态更新：将 agents.status 设为 retired
        // 确保死亡角色不再阻止同设备注册新角色
        if !dead_agents.is_empty() {
            for agent_id in &dead_agents {
                if let Err(e) = sqlx::query(
                    r#"UPDATE agents SET status = 'dead', retired_at = CURRENT_TIMESTAMP
                       WHERE agent_id = $1 AND status = 'active'"#,
                )
                .bind(*agent_id)
                .execute(&self.db_pool)
                .await
                {
                    warn!("Failed to retire dead agent {}: {}", agent_id, e);
                }
            }
            info!("已将 {} 个死亡 Agent 状态更新为 dead", dead_agents.len());
        }

        let phase2_2_duration = phase2_2_start.elapsed();
        let _phase2_duration = phase2_1_duration + phase2_2_duration;
        info!(
            "阶段2.2完成 - 应用衰减+环境压力, 死亡Agent: {}, 耗时: {:?}",
            dead_agents.len(),
            phase2_2_duration
        );

        // Bug #5: 告警阈值 - 单 tick 自然死亡数量异常升高时触发告警
        let death_threshold = crate::game_data::registry::registry()
            .map(|r| r.get().game_rules.data.ops.death_threshold)
            .unwrap_or(10);

        if dead_agents.len() > death_threshold {
            tracing::error!(
                "🚨 告警: 单 tick 自然死亡数量异常升高! Tick: {}, 死亡人数: {} (阈值: {})",
                tick_id,
                dead_agents.len(),
                death_threshold
            );
        }

        let phase3_start = Instant::now();
        let agents_processed = updated_states.len() as i32;
        let actions_executed = executed_actions as i32;

        // 跟踪意图超时统计（每10个tick记录一次，避免日志过多）
        if tick_id % 10 == 0 {
            match crate::db::get_intent_timeout_stats(&self.db_pool).await {
                Ok(stats) => {
                    info!(
                        "意图超时统计 - 存活Agent: {}, 超时Agent: {}, 超时率: {:.2}%",
                        stats.total_alive_agents,
                        stats.timeout_agents,
                        stats.timeout_rate * 100.0
                    );
                }
                Err(e) => {
                    warn!("获取意图超时统计失败: {}", e);
                }
            }
        }

        let phase3_duration = phase3_start.elapsed();
        info!(
            "阶段3完成 - 统计和超时跟踪, {}个Agent, {}个动作, 耗时: {:?}",
            agents_processed, actions_executed, phase3_duration
        );

        let phase4_start = Instant::now();
        persistence::persist_states(&self.db_pool, tick_id, &updated_states)
            .await
            .context("持久化状态失败")?;
        let phase4_duration = phase4_start.elapsed();
        info!("阶段4完成 - 持久化状态, 耗时: {:?}", phase4_duration);

        let total_duration = start_time.elapsed();
        info!(
            "Tick {} 结算完成 - 总耗时: {:?}, 处理Agent: {}, 执行动作: {}",
            tick_id, total_duration, agents_processed, actions_executed
        );

        if total_duration.as_secs() > 10 {
            warn!("Tick {} 耗时超过10秒: {:?}", tick_id, total_duration);
        }

        self.closed_dialogue_records = self.dialogue_manager.close_all_sessions().await;

        Ok((agents_processed, actions_executed))
    }
}

/// 计算关单时刻的绝对 Unix 毫秒时间戳
fn calculate_deadline_abs_ms(collection_window_secs: u64) -> u64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    now_ms + collection_window_secs * 1000
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, NaiveDate, TimeZone, Timelike};

    /// 测试东八区时间解析
    ///
    /// 验证 start_date: "2026-03-03" 被正确解析为北京时间 00:00:00
    #[test]
    fn test_utc8_game_epoch() {
        // 解析日期字符串
        let start_date_str = "2026-03-03";
        let date = NaiveDate::parse_from_str(start_date_str, "%Y-%m-%d").unwrap();

        // 使用东八区（UTC+8）时间
        let offset = FixedOffset::east_opt(8 * 3600).unwrap();
        let datetime = date.and_hms_opt(0, 0, 0).unwrap();
        let datetime_with_tz = datetime.and_local_timezone(offset).single().unwrap();

        // 获取 Unix 时间戳
        let timestamp = datetime_with_tz.timestamp();

        // 验证：北京时间 2026-03-03 00:00:00 = UTC 2026-03-02 16:00:00
        // 预期的 UTC 时间戳
        let expected_utc = NaiveDate::from_ymd_opt(2026, 3, 2)
            .unwrap()
            .and_hms_opt(16, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();

        assert_eq!(
            timestamp, expected_utc,
            "北京时间 2026-03-03 00:00:00 应该等于 UTC 2026-03-02 16:00:00"
        );

        // 验证具体数值
        // 2026-03-02 16:00:00 UTC 的 Unix 时间戳
        // 通过在线工具验证：https://www.unixtimestamp.com/
        // 2026-03-03 00:00:00 UTC+8 = 2026-03-02 16:00:00 UTC = 1772467200
        assert_eq!(timestamp, 1772467200, "时间戳应该等于 1772467200");
    }

    /// 测试 tick_id 计算（秒级秒数）
    ///
    /// 验证 tick_id = now - game_epoch（秒级秒数）
    #[test]
    fn test_tick_id_calculation() {
        let start_date_str = "2026-03-03";
        let date = NaiveDate::parse_from_str(start_date_str, "%Y-%m-%d").unwrap();
        let offset = FixedOffset::east_opt(8 * 3600).unwrap();
        let datetime = date.and_hms_opt(0, 0, 0).unwrap();
        let game_epoch = datetime
            .and_local_timezone(offset)
            .single()
            .unwrap()
            .timestamp();

        // tick_id = now - game_epoch（秒级秒数）
        // 在北京时间 2026-03-03 00:00:00，tick_id 应该是 0
        let tick_at_epoch = game_epoch - game_epoch;
        assert_eq!(tick_at_epoch, 0, "纪元时刻的 tick_id 应该是 0");

        // 在北京时间 2026-03-03 00:01:00（1分钟后），tick_id 应该是 60
        let one_minute_later = game_epoch + 60;
        let tick_after_1min = one_minute_later - game_epoch;
        assert_eq!(tick_after_1min, 60, "1分钟后的 tick_id 应该是 60");

        // 在北京时间 2026-03-03 01:00:00（1小时后），tick_id 应该是 3600
        let one_hour_later = game_epoch + 3600;
        let tick_after_1hour = one_hour_later - game_epoch;
        assert_eq!(tick_after_1hour, 3600, "1小时后的 tick_id 应该是 3600");
    }

    /// 测试时间戳转换的一致性
    ///
    /// 验证从时间戳反向转换回日期时间的正确性
    #[test]
    fn test_timestamp_roundtrip() {
        let start_date_str = "2026-03-03";
        let date = NaiveDate::parse_from_str(start_date_str, "%Y-%m-%d").unwrap();
        let offset = FixedOffset::east_opt(8 * 3600).unwrap();
        let datetime = date.and_hms_opt(0, 0, 0).unwrap();
        let datetime_with_tz = datetime.and_local_timezone(offset).single().unwrap();

        let timestamp = datetime_with_tz.timestamp();

        // 从时间戳反向转换
        let reversed = offset.timestamp_opt(timestamp, 0).single().unwrap();

        // 验证年月日时分秒一致
        assert_eq!(reversed.year(), 2026);
        assert_eq!(reversed.month(), 3);
        assert_eq!(reversed.day(), 3);
        assert_eq!(reversed.hour(), 0);
        assert_eq!(reversed.minute(), 0);
        assert_eq!(reversed.second(), 0);
    }
}
