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
use sha2::Digest;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::db::DbPool;
use crate::game_data::GameDataCache;
use crate::state::AgentStateCache;
use crate::websocket::{AgentToDeviceMap, ConnectionManager};

use super::WorkerMessage;
use super::broadcaster::Broadcaster;
use super::event_manager::SharedEventManager;

use crate::game_data::loaders::load_actions;
use crate::paths::get_config_dir;
use crate::websocket::broadcast_config_update;
use cyber_jianghu_protocol::{ConfigType, ServerMessage};

/// Tick调度器
///
/// 实时模式：Tick 退化为纯时钟（衰减 + 时间推进 + 周期广播 WorldState）。
/// Intent 由 IntentWorker 实时处理，不再经过 scheduler。
pub struct TickScheduler {
    /// 游戏数据缓存
    game_data_cache: Arc<GameDataCache>,

    /// 当前Tick编号（递增）
    current_tick_id: i64,

    /// Tick 计数器（每 tick +1，与墙钟解耦，用于边界判断）
    tick_counter: u64,

    /// 运行状态
    is_running: bool,

    /// 数据库连接池
    db_pool: DbPool,

    /// WebSocket 连接管理器
    connection_manager: ConnectionManager,

    /// agent_id → device_id 反向映射
    agent_to_device_map: AgentToDeviceMap,

    /// 事件管理器（与 IntentWorker 共享）
    event_manager: SharedEventManager,

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

    /// 上次加载的 skills/ 目录修改时间
    last_skills_mtime: Option<std::time::SystemTime>,

    /// 上次加载的 narrative_config.yaml 修改时间
    last_narrative_config_mtime: Option<std::time::SystemTime>,

    /// 上次加载的 game_rules.yaml 修改时间
    last_game_rules_mtime: Option<std::time::SystemTime>,

    /// 上次加载的 world_building_rules.yaml 修改时间
    last_world_building_rules_mtime: Option<std::time::SystemTime>,

    /// 上次加载的 prompt_templates.yaml 修改时间
    last_prompt_templates_mtime: Option<std::time::SystemTime>,

    /// Prompt 模板 JSON 缓存（与 AppState 共享，用于 WS 连接时下发）
    prompt_template_cache:
        Option<Arc<tokio::sync::RwLock<Option<crate::state::PromptTemplateCache>>>>,

    /// Vendor 跨请求事件缓冲（grant-items handler 写入，broadcast 消费）
    vendor_pending_events: crate::models::VendorPendingEvents,
}

/// 读取热重载配置文件的 metadata，集中编码"NotFound vs 真错"约定。
///
/// 约定（**根因级修复**，消除 12 个站点的 silent swallow）：
/// - `Ok(Some(SystemTime))`：文件存在，metadata 读成功，caller 继续比 mtime
/// - `Ok(None)`：NotFound = 文件真的不存在/未配置，无事可做，正常 skip
/// - `Err(...)`：NotFound 之外的 IO 错（PermissionDenied / InvalidInput / 文件锁 / 磁盘错）= 真错，
///   必须冒泡让 caller 决定（warn + propagate）
///
/// 之前 12 个 hot-reload 站点都用 `Err(_) => return Ok(())` 一刀切，把"权限被篡改"和
/// "文件未创建"混为"无事可做"，导致运维看不到 hot-reload 被攻击/磁盘满/锁文件等真问题。
pub(crate) fn read_file_metadata_for_hot_reload(
    path: &std::path::Path,
) -> anyhow::Result<Option<std::time::SystemTime>> {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "热重载 metadata 读取失败: path={}, err={}",
                path.display(),
                e
            ));
        }
    };
    let modified = metadata.modified().map_err(|e| {
        anyhow::anyhow!(
            "热重载 metadata.modified() 失败: path={}, err={}",
            path.display(),
            e
        )
    })?;
    Ok(Some(modified))
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
        vendor_pending_events: crate::models::VendorPendingEvents,
    ) -> Self {
        Self {
            game_data_cache,
            current_tick_id: 0,
            tick_counter: 0,
            is_running: false,
            db_pool,
            connection_manager,
            agent_to_device_map,
            event_manager: super::event_manager::EventManager::new_shared(),
            broadcaster: Broadcaster::new(),
            worker_tx,
            agent_state_cache,
            accepting_tick_id,
            last_actions_mtime: None,
            last_skills_mtime: None,
            last_narrative_config_mtime: None,
            last_game_rules_mtime: None,
            last_world_building_rules_mtime: None,
            last_prompt_templates_mtime: None,
            prompt_template_cache: None,
            vendor_pending_events,
        }
    }

    /// 设置 prompt_template_cache（与 AppState 共享）
    pub fn set_prompt_template_cache(
        &mut self,
        cache: Arc<tokio::sync::RwLock<Option<crate::state::PromptTemplateCache>>>,
    ) {
        self.prompt_template_cache = Some(cache);
    }
}

#[path = "scheduler_parts/reload_prompts_skills.rs"]
mod reload_prompts_skills;
#[path = "scheduler_parts/reload_rules.rs"]
mod reload_rules;
#[path = "scheduler_parts/run_loop.rs"]
mod run_loop;

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
