// ============================================================================
// 即时事件处理模块
// ============================================================================
//
// 处理 Server 下发的 ImmediateEvent（speak/whisper 等）
//
// 新架构：DB 持久化 + Session Triage LLM
//   Server -> ImmediateEvent -> EventStore SQLite INSERT -> Notify 信号
//   Session Triage Engine (后台任务) -> 批量 LLM triage -> 标记 urgent/batch/ignore
//   主 tick -> event_store.query_triaged() -> 注入 memory_context
//
// 与旧架构的区别：
//   - 无内存队列，事件持久化到 SQLite（零丢失）
//   - 无 per-event LLM 调用，Session Triage 批量处理
//   - 无 RespondNow，所有回应由主 tick 统一决策
//   - Notify 信号替代 mpsc channel（无容量限制）
// ============================================================================

pub mod event_store;
pub mod session_triage;

use std::sync::Arc;

use tracing::{debug, error, info};

use cyber_jianghu_protocol::ServerMessage;

use event_store::IncomingEvent;

// 公开导出供外部使用的类型
pub use event_store::{EventStore, StoredEvent, TriageDecision, TriageResult};
pub use session_triage::SessionTriageEngine;

// ============================================================================
// 即时事件处理器
// ============================================================================

/// 即时事件处理器（新架构）
///
/// 职责简化为：收消息 → DB 写入 + Notify 信号。
/// 不再做 LLM 调用，不做即时回应。
pub struct ImmediateEventHandler {
    /// EventStore（SQLite 持久化）
    event_store: Arc<EventStore>,

    /// 当前 tick_id（用于事件入库）
    current_tick_id: Arc<tokio::sync::RwLock<i64>>,

    /// 当前游戏日（用于事件入库）
    current_game_day: Arc<tokio::sync::RwLock<i64>>,
}

impl ImmediateEventHandler {
    /// 创建新的处理器
    pub fn new(event_store: Arc<EventStore>) -> Self {
        Self {
            event_store,
            current_tick_id: Arc::new(tokio::sync::RwLock::new(0)),
            current_game_day: Arc::new(tokio::sync::RwLock::new(0)),
        }
    }

    /// 获取 EventStore 引用（供 lifecycle.rs 读取 triage 结果）
    pub fn event_store(&self) -> &Arc<EventStore> {
        &self.event_store
    }

    /// 获取当前 game_day 的 Arc（供 SessionTriageEngine 共享）
    pub fn current_game_day(&self) -> Arc<tokio::sync::RwLock<i64>> {
        self.current_game_day.clone()
    }

    /// 更新当前 tick_id
    pub async fn set_tick_id(&self, tick_id: i64) {
        let mut guard = self.current_tick_id.write().await;
        *guard = tick_id;
    }

    /// 更新当前 game_day
    pub async fn set_game_day(&self, game_day: i64) {
        let mut guard = self.current_game_day.write().await;
        *guard = game_day;
    }

    /// 处理 Server 消息（提取 ImmediateEvent → DB 写入 + Notify）
    ///
    /// 纯 IO，无 LLM 调用。写入耗时 <1ms。
    pub async fn handle_server_message(&self, msg: ServerMessage) {
        if let ServerMessage::ImmediateEvent { event_id, event } = msg {
            let from_agent_id = event
                .metadata
                .get("from_agent_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let from_agent_name = event
                .metadata
                .get("from_agent_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let incoming = IncomingEvent {
                event_id,
                event_type: event.event_type,
                description: event.description,
                metadata: event.metadata,
                from_agent_id,
                from_agent_name,
            };

            let tick_id = *self.current_tick_id.read().await;
            let game_day = *self.current_game_day.read().await;

            if let Err(e) = self
                .event_store
                .insert_event_async(&incoming, tick_id, game_day)
                .await
            {
                error!("写入即时事件到 DB 失败: event_id={}, error={}", event_id, e);
            } else {
                debug!(
                    "即时事件已入库: event_id={}, type={}, game_day={}",
                    event_id,
                    incoming.event_type.as_str(),
                    game_day
                );
            }
        }
    }

    /// 停止处理器
    pub async fn stop(&self) {
        // 新架构下无需清理内存队列，DB 是持久化的
        info!("ImmediateEventHandler 已停止");
    }
}
