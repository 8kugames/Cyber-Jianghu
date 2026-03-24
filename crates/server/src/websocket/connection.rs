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

    /// Device ID（用于归属验证）
    #[allow(dead_code)]
    pub device_id: Uuid,

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
        device_id: Uuid,
        agent_name: String,
        sender: tokio::sync::mpsc::Sender<Message>,
    ) -> Self {
        Self {
            agent_id,
            device_id,
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

// ============================================================================
// agent_id → device_id 反向映射
// ============================================================================

/// agent_id → device_id 反向映射
///
/// 用于在角色注册后，通过 agent_id 找到对应的 device_id，
/// 从而找到正确的 WebSocket 连接
pub type AgentToDeviceMap = Arc<RwLock<HashMap<Uuid, Uuid>>>;

/// 创建 agent_id → device_id 映射表
pub fn create_agent_to_device_map() -> AgentToDeviceMap {
    Arc::new(RwLock::new(HashMap::new()))
}

/// 原子性地获取指定 tick 的 Intent
///
/// # 参数
/// - `intent_manager`: Intent 管理器
/// - `tick_id`: 当前 tick_id
///
/// # 返回
/// 返回当前 tick 的意图列表，移除所有不匹配的意图
pub async fn take_intents_for_tick(intent_manager: &IntentManager, tick_id: i64) -> Vec<Intent> {
    let mut intents_map = intent_manager.write().await;

    let mut current_tick_intents = Vec::new();
    let mut remove_agent_ids = Vec::new();

    for (agent_id, intent) in intents_map.iter() {
        if intent.tick_id == tick_id {
            current_tick_intents.push(intent.clone());
            remove_agent_ids.push(*agent_id);
        } else {
            tracing::debug!(
                "移除不匹配意图: agent={}, intent_tick={}, current_tick={}",
                agent_id,
                intent.tick_id,
                tick_id
            );
            remove_agent_ids.push(*agent_id);
        }
    }

    let removed_count = remove_agent_ids.len() - current_tick_intents.len();

    for agent_id in remove_agent_ids {
        intents_map.remove(&agent_id);
    }

    tracing::debug!(
        "🧹 Took {} intents for tick {}, removed {} mismatched intents",
        current_tick_intents.len(),
        tick_id,
        removed_count
    );

    current_tick_intents
}
