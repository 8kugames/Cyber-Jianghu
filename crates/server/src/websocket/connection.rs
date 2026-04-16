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

// ============================================================================
// 连接管理器
// ============================================================================

/// 连接管理器
///
/// 管理所有在线 Agent 的 WebSocket 连接
/// 使用 RwLock 支持并发读写
pub type ConnectionManager = Arc<RwLock<HashMap<Uuid, Connection>>>;

#[derive(Debug, Clone)]
pub struct Connection {
    pub agent_id: Uuid,
    pub device_id: Uuid,
    pub agent_name: String,
    pub sender: tokio::sync::mpsc::Sender<Message>,
    is_dead: bool,
}

impl Connection {
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
            is_dead: false,
        }
    }

    pub async fn send(&self, msg: Message) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.is_dead {
            return Err("Connection marked as dead".into());
        }
        if self.sender.try_send(msg).is_err() {
            return Err("Channel full or closed".into());
        }
        Ok(())
    }

    pub fn mark_dead(&mut self) {
        self.is_dead = true;
    }

    pub fn is_dead(&self) -> bool {
        self.is_dead
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 创建连接管理器
pub fn create_connection_manager() -> ConnectionManager {
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
