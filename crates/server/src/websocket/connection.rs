// ============================================================================
// WebSocket 连接管理
// ============================================================================
//
// 本模块管理 WebSocket 连接，包括：
// - 连接管理器（在线 Agent 列表）
// - 单个连接的状态和消息发送
// ============================================================================

use axum::extract::ws::Message;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::types::IntentManager;
use crate::models::Intent;

// ============================================================================
// 连接管理器
// ============================================================================

/// 连接管理器
///
/// 管理所有在线 Agent 的 WebSocket 连接
/// 使用 RwLock 支持并发读写
pub type ConnectionManager = Arc<RwLock<HashMap<Uuid, Connection>>>;

/// 单个连接
///
/// 包含 Agent 的 WebSocket 发送器
/// 用于向 Agent 发送消息
#[derive(Debug, Clone)]
pub struct Connection {
    /// Agent ID
    #[allow(dead_code)]
    pub agent_id: Uuid,

    /// Agent 名称
    #[allow(dead_code)]
    pub agent_name: String,

    /// WebSocket 发送器
    /// 使用 tokio::sync::mpsc::Sender 发送消息（带有背压）
    pub sender: tokio::sync::mpsc::Sender<Message>,
}

impl Connection {
    /// 创建新连接
    pub fn new(
        agent_id: Uuid,
        agent_name: String,
        sender: tokio::sync::mpsc::Sender<Message>,
    ) -> Self {
        Self {
            agent_id,
            agent_name,
            sender,
        }
    }

    /// 发送消息
    ///
    /// 尝试发送消息，如果通道已满则报错，避免阻塞 Tick 引擎
    pub async fn send(&self, msg: Message) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.sender.try_send(msg)?;
        Ok(())
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 创建连接管理器
pub fn create_connection_manager() -> ConnectionManager {
    Arc::new(RwLock::new(HashMap::new()))
}

/// 创建 Intent 管理器
pub fn create_intent_manager() -> IntentManager {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Intent tick_id 校验窗口大小
/// 允许当前 tick 或未来 ±TICK_WINDOW_SIZE 个 tick
const TICK_WINDOW_SIZE: i64 = 2;

/// 原子性地获取指定 tick 的 Intent，保留未来 tick 的 Intent
///
/// # 参数
/// - `intent_manager`: Intent 管理器
/// - `tick_id`: 当前 tick_id
///
/// # 返回
/// 返回当前 tick 的意图列表，同时：
/// - 移除已过期（< tick_id - TICK_WINDOW_SIZE）的意图
/// - 保留未来（> tick_id）的意图供后续 tick 使用
pub async fn take_intents_for_tick(intent_manager: &IntentManager, tick_id: i64) -> Vec<Intent> {
    let mut intents_map = intent_manager.write().await;

    let mut current_tick_intents = Vec::new();
    let mut expired_agent_ids = Vec::new();

    // 遍历所有缓存的意图
    for (agent_id, intent) in intents_map.iter() {
        let tick_diff = intent.tick_id - tick_id;

        if intent.tick_id == tick_id {
            // 当前 tick 的意图，收集并移除
            current_tick_intents.push(intent.clone());
            expired_agent_ids.push(*agent_id);
        } else if tick_diff < -TICK_WINDOW_SIZE {
            // 已过期（超出窗口的过去 tick），移除
            tracing::debug!(
                "移除过期意图: agent={}, intent_tick={}, current_tick={}",
                agent_id,
                intent.tick_id,
                tick_id
            );
            expired_agent_ids.push(*agent_id);
        }
        // 其他情况（未来 tick）保留在缓存中
    }

    // 移除已处理的意图
    for agent_id in expired_agent_ids {
        intents_map.remove(&agent_id);
    }

    tracing::debug!(
        "🧹 Took {} intents for tick {}, kept {} future intents",
        current_tick_intents.len(),
        tick_id,
        intents_map.len()
    );

    current_tick_intents
}
