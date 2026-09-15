// ============================================================================
// WebSocket 连接处理器
// ============================================================================
//
// 本模块处理 WebSocket 连接的生命周期，包括：
// - WebSocket 升级处理
// - 连接建立和初始化
// - 消息接收和处理
// - 连接清理
// ============================================================================

use anyhow::Context;
use axum::{
    body::Bytes,
    extract::{
        Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures_util::SinkExt;
use futures_util::stream::StreamExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tracing::{debug, error, info, warn};

use crate::dialogue::DialogueResponse;
use crate::game_data::registry::ActionRegistry;
use crate::game_data::registry::ItemRegistry;
use crate::game_data::types::actions::Transmission;
use crate::governance::ServerGovernanceMapper;
use crate::inventory::InventoryManager;
use crate::models::Intent;
use cyber_jianghu_protocol::{
    ClientMessage, DialogueMessage, GameError, ServerMessage, SoulCycleMetadata,
};

use super::broadcast;
use super::connection::Connection;
use super::types::{WebSocketQuery, build_game_rules_from_config, load_world_building_rules};

// ============================================================================
// WebSocket 升级处理
// ============================================================================

fn rebind_device_agent(
    agent_to_device: &mut std::collections::HashMap<uuid::Uuid, uuid::Uuid>,
    agent_id: uuid::Uuid,
    device_id: uuid::Uuid,
) {
    agent_to_device.retain(|existing_agent_id, existing_device_id| {
        *existing_device_id != device_id || *existing_agent_id == agent_id
    });
    agent_to_device.insert(agent_id, device_id);
}

/// WebSocket 升级处理器
///
/// GET /ws?device_id=xxx&token=yyy
///
/// 处理 WebSocket 升级请求，验证设备身份并建立连接
pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WebSocketQuery>,
    State(state): State<Arc<crate::state::AppState>>,
) -> Response {
    // 调试日志：显示收到的参数
    debug!(
        "WebSocket request: device_id={}, token={}",
        query.device_id, query.token
    );

    // 1. 验证设备身份（device_id + auth_token）
    let device_valid =
        match crate::db::verify_device_token(&state.db_pool, query.device_id, &query.token).await {
            Ok(valid) => valid,
            Err(e) => {
                // 输出完整的错误链
                let mut err_msg = format!("{}", e);
                let mut source = e.source();
                while let Some(s) = source {
                    err_msg.push_str(&format!("\n  Caused by: {}", s));
                    source = s.source();
                }
                warn!("Device verification error: {}", err_msg);
                return Response::builder()
                    .status(500)
                    .body("Internal server error".into())
                    .expect("valid HTTP response");
            }
        };

    if !device_valid {
        warn!("Invalid device credentials: device_id={}", query.device_id);
        return Response::builder()
            .status(401)
            .body("Unauthorized".into())
            .expect("valid HTTP response");
    }

    // 2. 更新设备最后在线时间
    if let Err(e) = crate::db::update_device_last_seen(&state.db_pool, query.device_id).await {
        warn!("Failed to update device last_seen: {}", e);
    }

    // 3. 获取该设备的角色信息（从数据库查询）
    // 统一通过 device_id 查找最新角色，避免指定 retired agent_id 导致 nil
    let agent_id = match crate::db::get_agent_by_device_id(&state.db_pool, query.device_id).await {
        Ok(Some(agent)) => {
            if agent.retired_at.is_some() {
                info!(
                    "Device {} has retired agent {}, waiting for new registration",
                    query.device_id, agent.agent_id
                );
                uuid::Uuid::nil()
            } else {
                info!(
                    "Device {} has agent '{}' ({})",
                    query.device_id, agent.name, agent.agent_id
                );
                agent.agent_id
            }
        }
        Ok(None) => {
            info!(
                "Device {} connected without agent, waiting for character registration",
                query.device_id
            );
            uuid::Uuid::nil()
        }
        Err(e) => {
            warn!("Failed to query agent by device_id: {}", e);
            uuid::Uuid::nil()
        }
    };

    // 4. 获取 Agent 名称（如果有）
    let agent_name = if agent_id != uuid::Uuid::nil() {
        match crate::db::get_agent_by_id(&state.db_pool, agent_id).await {
            Ok(agent) => agent.name,
            Err(_) => "Unknown".to_string(),
        }
    } else {
        "Pending".to_string()
    };

    info!(
        "Device {} (agent: {}) requesting WebSocket connection",
        query.device_id, agent_id
    );

    // 升级到 WebSocket
    ws.max_message_size(1024 * 1024) // 1MB limit
        .max_frame_size(1024 * 1024)
        .on_upgrade(move |socket| {
            handle_websocket(socket, agent_id, query.device_id, agent_name, state)
        })
}

mod connection;
mod dialogue;
mod intent;
mod inventory;
mod messages;
mod reports;

// 供 websocket_handler 与各子模块互相调用（子模块经 use super::* 引入）
use connection::handle_websocket;
use dialogue::handle_dialogue_message;
use intent::handle_intent;
use messages::handle_client_message;
use reports::{handle_daily_summary, handle_relationship_snapshot, handle_soul_cycle_report};

pub(crate) use inventory::{load_initial_inventory, load_nearby_ground_items};

#[cfg(test)]
#[path = "handler/handler_tests.rs"]
mod tests;
