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
use tracing::{debug, info, warn};

use super::connection::{AgentToDeviceMap, ConnectionManager};

// ============================================================================
// 消息广播函数
// ============================================================================

pub async fn send_world_state(
    agent_id: uuid::Uuid,
    world_state: crate::models::WorldState,
    connection_manager: &ConnectionManager,
    agent_to_device_map: &AgentToDeviceMap,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let device_id = {
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

    let mut connections = connection_manager.write().await;
    if let Some(connection) = connections.get_mut(&device_id) {
        if connection.is_dead() {
            warn!(
                "Agent {} connection is dead, skipping WorldState send",
                agent_id
            );
            return Ok(());
        }
        let msg = ServerMessage::WorldState { data: world_state };
        let json = serde_json::to_string(&msg)?;
        if connection.send(Message::Text(json.into())).await.is_err() {
            connection.mark_dead();
            warn!("Agent {} send failed, marking connection as dead", agent_id);
            return Ok(());
        }
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

pub async fn forward_dialogue_message(
    to_agent_id: uuid::Uuid,
    message: DialogueMessage,
    connection_manager: &ConnectionManager,
    agent_to_device_map: &AgentToDeviceMap,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let device_id = {
        let agent_to_device = agent_to_device_map.read().await;
        match agent_to_device.get(&to_agent_id) {
            Some(&device_id) => device_id,
            None => {
                warn!(
                    "Agent {} has no device mapping, cannot send dialogue",
                    to_agent_id
                );
                return Err("Agent not online".into());
            }
        }
    };

    let mut connections = connection_manager.write().await;
    if let Some(connection) = connections.get_mut(&device_id) {
        if connection.is_dead() {
            warn!(
                "Agent {} connection is dead, cannot send dialogue",
                to_agent_id
            );
            return Err("Connection dead".into());
        }
        let msg = ServerMessage::Dialogue { message };
        let json = serde_json::to_string(&msg)?;
        if connection.send(Message::Text(json.into())).await.is_err() {
            connection.mark_dead();
            warn!("Agent {} dialogue send failed, marking dead", to_agent_id);
            return Err("Send failed".into());
        }
        debug!("对话消息已发送给 agent {}", to_agent_id);
    } else {
        warn!(
            "Agent {} is not online (device {} not connected)",
            to_agent_id, device_id
        );
        return Err("Agent not online".into());
    }

    Ok(())
}

/// 死亡通知上下文（用于减少函数参数）
pub struct DeathNotificationContext<'a> {
    pub connection_manager: &'a ConnectionManager,
    pub agent_to_device_map: &'a AgentToDeviceMap,
}

/// 向指定 Agent 发送死亡通知
pub async fn send_agent_died_notification(
    agent_id: uuid::Uuid,
    cause: String,
    description: String,
    location: String,
    tick_id: i64,
    died_at: i64,
    ctx: &DeathNotificationContext<'_>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let device_id = {
        let agent_to_device = ctx.agent_to_device_map.read().await;
        match agent_to_device.get(&agent_id) {
            Some(&device_id) => device_id,
            None => {
                warn!(
                    "Agent {} has no device mapping, cannot send AgentDied notification",
                    agent_id
                );
                return Ok(());
            }
        }
    };

    let mut connections = ctx.connection_manager.write().await;

    if let Some(connection) = connections.get_mut(&device_id) {
        if connection.is_dead() {
            warn!(
                "Agent {} connection is dead, cannot send AgentDied notification",
                agent_id
            );
            return Ok(());
        }
        let msg = ServerMessage::AgentDied {
            agent_id,
            cause,
            description,
            location,
            tick_id,
            died_at,
            rebirth_delay_ticks: 0,
        };
        let json = serde_json::to_string(&msg)?;
        if connection.send(Message::Text(json.into())).await.is_err() {
            connection.mark_dead();
            warn!(
                "Agent {} AgentDied send failed, marking connection dead",
                agent_id
            );
        } else {
            debug!(
                "AgentDied notification sent to agent {} via device {}",
                agent_id, device_id
            );
        }
    } else {
        warn!(
            "Agent {} is not online (device {} not connected), cannot send AgentDied notification",
            agent_id, device_id
        );
    }

    Ok(())
}

/// 广播动作配置更新到所有在线 Agent
pub async fn broadcast_action_update(
    action_update: ServerMessage,
    connection_manager: &ConnectionManager,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let connections = connection_manager.read().await;
    let mut success_count = 0;
    let mut fail_count = 0;

    for (device_id, connection) in connections.iter() {
        if connection.is_dead() {
            warn!(
                "Agent {} connection is dead, skipping ActionUpdate",
                connection.agent_id
            );
            fail_count += 1;
            continue;
        }

        let msg = action_update.clone();

        let json = serde_json::to_string(&msg)?;
        if connection.send(Message::Text(json.into())).await.is_err() {
            warn!(
                "Agent {} ActionUpdate send failed, marking connection dead",
                connection.agent_id
            );
            fail_count += 1;
        } else {
            debug!(
                "ActionUpdate sent to agent {} via device {}",
                connection.agent_id, device_id
            );
            success_count += 1;
        }
    }

    info!(
        "ActionUpdate broadcast complete: {} success, {} failed",
        success_count, fail_count
    );
    Ok(())
}
