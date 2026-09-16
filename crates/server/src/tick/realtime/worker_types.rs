// ============================================================================
// WorkerMessage 与 IntentWorker 结构体/构造/主循环
// ============================================================================

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::db::DbPool;
use crate::dialogue::DialogueManager;
use crate::game_data::GameDataCache;
use crate::game_data::types::actions::Transmission;
use crate::state::AgentStateCache;

use super::super::processor::StateProcessor;

// ============================================================================
//
// IntentWorker 是单消费者事件循环，顺序处理两类消息：
// - Intent: Agent 提交的意图，立即验证+执行+持久化+广播结果
// - TickBoundary: Tick 周期信号，执行衰减+批量持久化+广播 WorldState
//
// 设计原则：
// - 单消费者消除所有竞态（DashMap 不存在并发写入冲突）
// - write-through: persist 到 DB 确认后才更新 DashMap
// - 非阻塞: handler.rs 用 try_send，队列满时返回错误而非 block

// ============================================================================
// Worker Message 枚举
// ============================================================================

/// IntentWorker 消息类型
///
/// 单一 channel 传递两种消息，保证顺序处理、零竞态。
pub enum WorkerMessage {
    /// Agent 提交的意图（实时处理）
    Intent {
        intent: Box<cyber_jianghu_protocol::Intent>,
    },
    /// Tick 周期边界信号（衰减 + 广播）
    TickBoundary { tick_id: i64 },
}

// ============================================================================
// IntentWorker
// ============================================================================

/// 实时 Intent 处理引擎
pub struct IntentWorker {
    /// 数据库连接池
    pub(crate) db_pool: DbPool,
    /// Agent 状态内存缓存
    pub(crate) state_cache: AgentStateCache,
    /// 最近成功执行 intent 的 tick（agent_id → tick_id，秒级时间戳）
    ///
    /// 唯一写入点：process_intent 持久化成功后；
    /// 唯一读取点：process_tick_boundary 衰减前的休息判定
    /// （tick_id 差值在 real_seconds_per_tick 秒内视为本 tick 窗口内行动过）。
    pub(crate) last_intent_ticks: dashmap::DashMap<uuid::Uuid, i64>,
    /// 状态处理器（验证 + 执行 + 状态变更）
    pub(crate) state_processor: Arc<StateProcessor>,
    /// WebSocket 连接管理器（广播用）
    pub(crate) connection_manager: ConnectionManager,
    /// agent_id → device_id 映射（广播用）
    pub(crate) agent_to_device_map: AgentToDeviceMap,
    /// 对话管理器（whisper session 生命周期管理）
    pub(crate) dialogue_manager: Arc<DialogueManager>,
    /// 游戏数据缓存（构建 WorldState 用）
    pub(crate) game_data_cache: Arc<GameDataCache>,
}

use tracing::{error, info, warn};

use crate::game_data::registry::ActionRegistry;
use crate::websocket::AgentToDeviceMap;
use crate::websocket::ConnectionManager;

impl IntentWorker {
    pub fn new(
        db_pool: DbPool,
        state_cache: AgentStateCache,
        state_processor: Arc<StateProcessor>,
        connection_manager: ConnectionManager,
        agent_to_device_map: AgentToDeviceMap,
        dialogue_manager: Arc<DialogueManager>,
        game_data_cache: Arc<GameDataCache>,
    ) -> Self {
        Self {
            db_pool,
            state_cache,
            last_intent_ticks: dashmap::DashMap::new(),
            state_processor,
            connection_manager,
            agent_to_device_map,
            dialogue_manager,
            game_data_cache,
        }
    }

    pub(super) async fn close_session_if_whisper(
        &self,
        action_type: &str,
        intent: &cyber_jianghu_protocol::Intent,
    ) {
        let is_session = ActionRegistry::get(action_type)
            .map(|c| c.transmission == Transmission::Session)
            .unwrap_or(false);
        if is_session && let Some(ref session_id) = intent.session_id {
            self.dialogue_manager.close_session(session_id).await;
        }
    }

    /// 启动 Worker 事件循环
    ///
    /// 消费 MPSC channel 直到发送端关闭（server shutdown）。
    pub async fn run(self, mut rx: mpsc::Receiver<WorkerMessage>) {
        info!("IntentWorker 启动");

        while let Some(msg) = rx.recv().await {
            match msg {
                WorkerMessage::Intent { intent } => {
                    if let Err(e) = self.process_intent(*intent).await {
                        // 错误已在 process_intent 内部记录
                        warn!("Intent 处理失败: {}", e);
                    }
                }
                WorkerMessage::TickBoundary { tick_id } => {
                    if let Err(e) = self.process_tick_boundary(tick_id).await {
                        error!("Tick {} 边界处理失败: {}", tick_id, e);
                    }
                }
            }
        }

        info!("IntentWorker 停止（channel 关闭）");
    }

    // ========================================================================
    // Intent 处理
    // ========================================================================
}
