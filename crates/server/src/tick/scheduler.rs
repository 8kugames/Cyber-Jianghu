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
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::db::DbPool;
use crate::models::TickLog;
use crate::websocket::{ConnectionManager, IntentManager};

use super::super::inventory::InventoryManager;
use super::broadcaster::Broadcaster;
use super::event_manager::EventManager;
use super::intent_collector::IntentCollector;
use super::{decay, persistence, state_processor};

/// Tick调度器
///
/// 负责驱动游戏世界的运行
pub struct TickScheduler {
    /// 配置
    config: Config,

    /// 当前Tick编号（递增）
    current_tick_id: i64,

    /// 运行状态
    is_running: bool,

    /// 数据库连接池
    db_pool: DbPool,

    /// WebSocket 连接管理器
    connection_manager: ConnectionManager,

    /// Intent 管理器（临时缓存）
    intent_manager: IntentManager,

    /// 事件管理器
    event_manager: EventManager,

    /// 意图收集器
    intent_collector: IntentCollector,

    /// 广播器
    broadcaster: Broadcaster,
}

impl TickScheduler {
    /// 创建新的Tick调度器
    pub fn new(
        config: Config,
        db_pool: DbPool,
        connection_manager: ConnectionManager,
        intent_manager: IntentManager,
    ) -> Self {
        Self {
            config,
            current_tick_id: 0,
            is_running: false,
            db_pool,
            connection_manager,
            intent_manager,
            event_manager: EventManager::new(),
            intent_collector: IntentCollector::new(),
            broadcaster: Broadcaster::new(),
        }
    }

    /// 启动Tick循环
    ///
    /// 这是一个无限循环，直到收到停止信号
    pub async fn run(&mut self) -> Result<()> {
        info!(
            "Tick引擎启动，周期: {}秒",
            self.config.tick_engine.tick_duration_secs
        );
        info!("天道无为，万物自化。世界开始运转。");

        self.is_running = true;

        // 获取当前最大的 tick_id，作为起始点
        let max_tick_id = crate::db::get_current_world_tick_id(&self.db_pool)
            .await
            .unwrap_or(0);
        
        self.current_tick_id = max_tick_id + 1;
        info!("当前起始 Tick ID: {}", self.current_tick_id);

        let mut interval = tokio::time::interval(Duration::from_secs(
            self.config.tick_engine.tick_duration_secs,
        ));
        
        // 设置错开第一次无延迟的 tick（如果有需要）
        // interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // 主循环
        while self.is_running {
            interval.tick().await;

            // 执行一次Tick（F-06：失败时写入 tick_logs）
            if let Err(e) = self.execute_tick().await {
                error!("Tick {} 执行失败: {}", self.current_tick_id, e);
                // 不要因为一次失败就停止整个引擎
                // 继续下一个Tick
            }

            self.current_tick_id += 1;
        }

        info!("Tick引擎已停止");
        Ok(())
    }

    /// 执行一次Tick
    ///
    /// 这是Tick引擎的核心方法，包含完整的Tick执行流程
    async fn execute_tick(&mut self) -> Result<()> {
        let tick_id = self.current_tick_id;
        let mut tick_log = TickLog::new(tick_id);
        info!("Tick {} 开始执行", tick_id);

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
        tick_log: &mut TickLog,
    ) -> Result<(i32, i32)> {
        let start_time = Instant::now();

        let agents_processed;
        let actions_executed;

        self.event_manager.clear();

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

        let phase2_start = Instant::now();

        // 1. 记录系统事件：当前时间/季节广播
        let mut time_events = Vec::new();
        if let Some(time_display) = crate::game_data::registry::TimeRegistry::get_time_display(tick_id) {
            // 每当进入新的一小时（整点）或者新的一天时，可以广播系统消息
            let is_new_hour = tick_id % crate::game_data::registry::TimeRegistry::get_config().map(|c| c.ticks_per_hour as i64).unwrap_or(60) == 0;
            
            if is_new_hour {
                let season_name = time_display.season.map(|s| s.name).unwrap_or_else(|| "未知".to_string());
                let time_desc = format!("现在是 {} 季，第 {} 天，{} 时", season_name, time_display.day, time_display.hour);
                
                // 将时间信息添加到每个 Agent 的事件列表中（作为全局环境信息）
                for state in &agent_states {
                    if state.is_alive {
                        let event = crate::models::WorldEvent {
                            event_type: "time_update".to_string(),
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

        // 2. 处理自然衰减和环境伤害
        let (mut updated_states, dead_agents, mut decay_events) =
            decay::apply_decay_and_environmental_damage(&self.config, tick_id, agent_states);
            
        // 将时间事件合并到衰减事件中
        decay_events.extend(time_events);

        for (agent_id, event) in decay_events {
            self.event_manager.add_event_for_agent(agent_id, event);
        }

        if !dead_agents.is_empty() {
            for agent_id in &dead_agents {
                let already_cleared = updated_states.iter()
                    .any(|s| s.agent_id == *agent_id && s.inventory_cleared_this_tick);

                if !already_cleared {
                    // 获取死亡 Agent 的位置用于掉落
                    let location = updated_states.iter()
                        .find(|s| s.agent_id == *agent_id)
                        .map(|s| s.node_id.clone());

                    match InventoryManager::clear_inventory(&self.db_pool, *agent_id).await {
                        Ok(items) => {
                            // 死亡掉落物品到地面
                            if let Some(loc) = &location {
                                for item in &items {
                                    if let Err(e) = crate::db::add_ground_item(&self.db_pool, loc, &item.item_id, item.quantity, Some(*agent_id)).await {
                                        warn!("自然死亡掉落物品添加到地面失败: {}", e);
                                    }
                                }
                            }
                            info!("Agent {} 自然死亡，背包已清空并掉落 {} 个物品到地面", agent_id, items.len());
                        }
                        Err(e) => {
                            warn!("清空死亡Agent {} 背包失败: {}", agent_id, e);
                        }
                    }

                    // 标记已清空，防止后续重复处理
                    if let Some(state) = updated_states.iter_mut().find(|s| s.agent_id == *agent_id) {
                        state.inventory_cleared_this_tick = true;
                    }
                }
            }
        }

        let phase2_duration = phase2_start.elapsed();
        info!(
            "阶段2完成 - 应用衰减+环境压力, 死亡Agent: {}, 耗时: {:?}",
            dead_agents.len(),
            phase2_duration
        );

        let phase3_start = Instant::now();
        let intents = self
            .intent_collector
            .collect_intents(&self.intent_manager, tick_id)
            .await
            .context("收集意图失败")?;
        let (resolved_states, executed_actions, processor_events, action_logs) =
            state_processor::resolve_intents(&self.db_pool, tick_id, updated_states, &intents)
                .await
                .context("结算意图失败")?;

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

        let phase3_duration = phase3_start.elapsed();
        agents_processed = resolved_states.len() as i32;
        actions_executed = executed_actions as i32;

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

        info!(
            "阶段3完成 - 结算意图: {}个意图, {}个Agent, {}个动作, 耗时: {:?}",
            intents.len(),
            agents_processed,
            actions_executed,
            phase3_duration
        );

        let phase4_start = Instant::now();
        persistence::persist_states(&self.db_pool, tick_id, &resolved_states)
            .await
            .context("持久化状态失败")?;
        let phase4_duration = phase4_start.elapsed();
        info!("阶段4完成 - 持久化状态, 耗时: {:?}", phase4_duration);

        let phase5_start = Instant::now();
        self.broadcaster
            .broadcast_states(
                tick_id,
                &resolved_states,
                &self.db_pool,
                &self.connection_manager,
                &self.event_manager,
            )
            .await
            .context("广播状态失败")?;
        let phase5_duration = phase5_start.elapsed();
        info!("阶段5完成 - 广播状态, 耗时: {:?}", phase5_duration);

        let total_duration = start_time.elapsed();
        info!(
            "Tick {} 完成 - 总耗时: {:?}, 处理Agent: {}, 执行动作: {}",
            tick_id, total_duration, agents_processed, actions_executed
        );

        if total_duration.as_secs() > 10 {
            warn!("Tick {} 耗时超过10秒: {:?}", tick_id, total_duration);
        }

        Ok((agents_processed, actions_executed))
    }
}
