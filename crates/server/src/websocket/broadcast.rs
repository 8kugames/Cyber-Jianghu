// ============================================================================
// WebSocket 消息广播
// ============================================================================
//
// 本模块处理消息广播功能，包括：
// - 向单个 Agent 发送 WorldState
// - 向所有在线 Agent 广播 WorldState
// - 广播游戏规则更新
// - 广播世界观规则更新
// ============================================================================

use axum::extract::ws::Message;
use cyber_jianghu_protocol::{DialogueMessage, ServerMessage};
use tracing::{debug, warn};

use super::connection::{AgentToDeviceMap, ConnectionManager};

// ============================================================================
// 消息广播函数
// ============================================================================

/// 向指定 Agent 发送 WorldState
///
/// 通过 agent_id 查找对应的 device_id，再找到 WebSocket 连接并发送
pub async fn send_world_state(
    agent_id: uuid::Uuid,
    world_state: crate::models::WorldState,
    connection_manager: &ConnectionManager,
    agent_to_device_map: &AgentToDeviceMap,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let connections = connection_manager.read().await;

    let device_id = if let Some(conn) = connections.get(&agent_id) {
        conn.device_id
    } else {
        let agent_to_device = agent_to_device_map.read().await;
        match agent_to_device.get(&agent_id) {
            Some(&device_id) => device_id,
            None => {
                warn!(
                    "Agent {} is not online and no device mapping found",
                    agent_id
                );
                return Ok(());
            }
        }
    };

    if let Some(connection) = connections.get(&device_id) {
        let protocol_world_state = world_state;
        let msg = ServerMessage::WorldState {
            data: protocol_world_state,
        };
        let json = serde_json::to_string(&msg)?;
        connection.send(Message::Text(json.into())).await?;
        debug!(
            "WorldState sent to agent {} via device {}",
            agent_id, device_id
        );
    } else {
        warn!(
            "Agent {} is not online (device {} not connected)",
            agent_id, device_id
        );
    }

    Ok(())
}

/// 转发对话消息给指定 Agent
///
/// 通过 WebSocket 连接发送对话消息
pub async fn forward_dialogue_message(
    to_agent_id: uuid::Uuid,
    message: DialogueMessage,
    connection_manager: &ConnectionManager,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let connections = connection_manager.read().await;

    if let Some(connection) = connections.get(&to_agent_id) {
        let msg = ServerMessage::Dialogue { message };
        let json = serde_json::to_string(&msg)?;
        connection.send(Message::Text(json.into())).await?;
        debug!("对话消息已发送给 agent {}", to_agent_id);
    } else {
        warn!("Agent {} 不在线，无法发送对话消息", to_agent_id);
        return Err("Agent not online".into());
    }

    Ok(())
}

/// 向指定 Agent 发送死亡通知
///
/// 通过 WebSocket 连接发送 AgentDied 消息
pub async fn send_agent_died_notification(
    agent_id: uuid::Uuid,
    cause: String,
    description: String,
    location: String,
    tick_id: i64,
    died_at: i64,
    connection_manager: &ConnectionManager,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let connections = connection_manager.read().await;

    if let Some(connection) = connections.get(&agent_id) {
        let msg = ServerMessage::AgentDied {
            agent_id,
            cause,
            description,
            location,
            tick_id,
            died_at,
            rebirth_delay_ticks: 0, // Current design: immediate rebirth
        };
        let json = serde_json::to_string(&msg)?;
        connection.send(Message::Text(json.into())).await?;
        debug!("AgentDied notification sent to agent {}", agent_id);
    } else {
        warn!(
            "Agent {} is not online, cannot send AgentDied notification",
            agent_id
        );
    }

    Ok(())
}
