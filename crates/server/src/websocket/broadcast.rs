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

use super::connection::ConnectionManager;

// ============================================================================
// 消息广播函数
// ============================================================================

/// 向指定 Agent 发送 WorldState
pub async fn send_world_state(
    agent_id: uuid::Uuid,
    world_state: crate::models::WorldState,
    connection_manager: &ConnectionManager,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let connections = connection_manager.read().await;

    if let Some(connection) = connections.get(&agent_id) {
        let protocol_world_state = cyber_jianghu_protocol::WorldState::from(world_state);
        let msg = ServerMessage::WorldState {
            data: protocol_world_state,
        };
        let json = serde_json::to_string(&msg)?;
        connection.send(Message::Text(json.into())).await?;
        debug!("WorldState sent to agent {}", agent_id);
    } else {
        warn!("Agent {} is not online, cannot send WorldState", agent_id);
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
