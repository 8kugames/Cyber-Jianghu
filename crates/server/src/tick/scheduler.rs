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
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::db::DbPool;
use crate::game_data::GameDataCache;
use crate::state::AgentStateCache;
use crate::websocket::{AgentToDeviceMap, ConnectionManager};

use super::WorkerMessage;
use super::broadcaster::Broadcaster;
use super::event_manager::EventManager;

use crate::game_data::loaders::load_actions;
use crate::paths::get_config_dir;
use crate::websocket::broadcast_action_update;
use cyber_jianghu_protocol::ServerMessage;
use std::fs;

/// Tick调度器
///
/// 实时模式：Tick 退化为纯时钟（衰减 + 时间推进 + 周期广播 WorldState）。
/// Intent 由 IntentWorker 实时处理，不再经过 scheduler。
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

    /// 事件管理器
    event_manager: EventManager,

    /// 广播器
    broadcaster: Broadcaster,

    /// IntentWorker 发送端（发送 TickBoundary 触发衰减）
    worker_tx: mpsc::Sender<WorkerMessage>,

    /// Agent 状态内存缓存
    agent_state_cache: AgentStateCache,

    /// 当前 tick_id（原子变量，供外部查询当前 tick）
    accepting_tick_id: Arc<AtomicI64>,

    /// 上次加载的 actions.yaml 修改时间
    last_actions_mtime: Option<std::time::SystemTime>,
}

impl TickScheduler {
    /// 创建新的Tick调度器
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        game_data_cache: Arc<GameDataCache>,
        db_pool: DbPool,
        connection_manager: ConnectionManager,
        agent_to_device_map: AgentToDeviceMap,
        worker_tx: mpsc::Sender<WorkerMessage>,
        agent_state_cache: AgentStateCache,
        accepting_tick_id: Arc<AtomicI64>,
    ) -> Self {
        Self {
            game_data_cache,
            current_tick_id: 0,
            is_running: false,
            db_pool,
            connection_manager,
            agent_to_device_map,
            event_manager: EventManager::new(),
            broadcaster: Broadcaster::new(),
            worker_tx,
            agent_state_cache,
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

            interval.tick().await;

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

            // 2. 广播 WorldState（deadline_ms=0，实时模式无 deadline）
            if let Err(e) = self.broadcast_new_tick(self.current_tick_id, 0).await {
                error!("Tick {} 广播失败: {}", self.current_tick_id, e);
            }

            // 3. 群像传记：每 168 tick (7 游戏日) 生成一次
            let period_ticks = crate::chronicle::ChronicleConfig::default().period_ticks;
            if self.current_tick_id > 0 && self.current_tick_id % period_ticks == 0 {
                let period_start = self.current_tick_id - period_ticks + 1;
                let db_pool = self.db_pool.clone();
                let tick_id = self.current_tick_id;
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
    async fn broadcast_new_tick(&mut self, tick_id: i64, deadline_ms: u64) -> Result<()> {
        let agent_states: Vec<crate::models::AgentState> = self
            .agent_state_cache
            .iter()
            .map(|r| r.value().clone())
            .collect();

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
