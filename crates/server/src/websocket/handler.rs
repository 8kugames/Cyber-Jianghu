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
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures_util::SinkExt;
use futures_util::stream::StreamExt;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::dialogue::DialogueResponse;
use crate::game_data::registry::ItemRegistry;
use crate::inventory::InventoryManager;
use crate::models::Intent;
use cyber_jianghu_protocol::{ClientMessage, DialogueMessage, ServerMessage};

use super::connection::Connection;
use super::types::{WebSocketQuery, build_game_rules_from_config, load_world_building_rules};

// ============================================================================
// WebSocket 升级处理
// ============================================================================

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
                    .unwrap();
            }
        };

    if !device_valid {
        warn!("Invalid device credentials: device_id={}", query.device_id);
        return Response::builder()
            .status(401)
            .body("Unauthorized".into())
            .unwrap();
    }

    // 2. 更新设备最后在线时间
    if let Err(e) = crate::db::update_device_last_seen(&state.db_pool, query.device_id).await {
        warn!("Failed to update device last_seen: {}", e);
    }

    // 3. 获取该设备的角色信息（从数据库查询）
    let agent_id = match crate::db::get_agent_by_device_id(&state.db_pool, query.device_id).await {
        Ok(Some(agent)) => {
            info!(
                "Device {} has agent '{}' ({})",
                query.device_id, agent.name, agent.agent_id
            );
            agent.agent_id
        }
        Ok(None) => {
            // 设备验证通过但没有角色，允许连接但标记为待注册状态
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
    ws.on_upgrade(move |socket| {
        handle_websocket(socket, agent_id, query.device_id, agent_name, state)
    })
}

// ============================================================================
// WebSocket 消息处理
// ============================================================================

/// 处理 WebSocket 连接
///
/// 这个函数在 WebSocket 连接建立后运行，负责：
/// 1. 添加连接到在线列表
/// 2. 接收来自 Agent 的消息
/// 3. 处理消息（Intent 上报）
/// 4. 清理连接（断开时）
async fn handle_websocket(
    socket: WebSocket,
    agent_id: uuid::Uuid,
    device_id: uuid::Uuid,
    agent_name: String,
    state: Arc<crate::state::AppState>,
) {
    info!(
        "WebSocket connected for agent '{}' ({})",
        agent_name, agent_id
    );

    // 分离 WebSocket 的发送和接收端（提前分离，以便在拒绝时使用）
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // agent_id 为零 = 角色已归隐或未注册，但设备验证通过
    // 允许连接，让 Agent 可以注册新角色（通过 /api/v1/agent/register）
    let is_pending_registration = agent_id == uuid::Uuid::nil();
    if is_pending_registration {
        info!(
            "Device {} connected for pending registration (character retired or new device)",
            device_id
        );
    }

    // 创建消息通道（用于向 Agent 发送消息），限制容量以提供背压
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(100);

    // 添加到连接管理器（使用 device_id 作为 key）
    {
        let mut connections = state.connection_manager.write().await;
        let connection = Connection::new(agent_id, device_id, agent_name.clone(), tx.clone());
        connections.insert(device_id, connection);
        info!(
            "Agent '{}' added to online list (device={}). Total online: {}",
            agent_name,
            device_id,
            connections.len()
        );
    }

    // 更新 agent_id → device_id 反向映射（用于 WebSocket 广播）
    // 重要：WebSocket 重连时需要更新映射，因为 agent_register 只在首次注册时调用
    if agent_id != uuid::Uuid::nil() {
        let mut agent_to_device = state.agent_to_device_map.write().await;
        agent_to_device.insert(agent_id, device_id);
        info!(
            "Updated agent_to_device_map on WebSocket connect: {} → {}",
            agent_id, device_id
        );
    }

    // 查询角色存活状态（如果有角色）
    let is_alive = if agent_id != uuid::Uuid::nil() {
        match crate::db::get_latest_agent_state(&state.db_pool, agent_id).await {
            Ok(agent_state) => agent_state.is_alive,
            Err(e) => {
                warn!("Failed to query agent state for is_alive check: {}", e);
                true // 查询失败默认存活，避免误判死亡
            }
        }
    } else {
        false // nil agent_id = 无角色 = 不存活
    };

    // 准备注册成功消息（包含游戏规则）
    let registered_json = {
        // 从配置构建 GameRules
        let gd = state.game_data.get();
        let tick_duration_secs = gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64;
        let survival_threshold = gd.game_rules.data.agent_state.survival.critical_threshold;
        let game_rules_version = gd.game_rules.version.clone();
        drop(gd);

        let game_rules = build_game_rules_from_config(
            tick_duration_secs,
            survival_threshold,
            game_rules_version,
        );

        // 加载世界观规则（可选）
        let world_building_rules = load_world_building_rules();

        let registered_msg = ServerMessage::Registered {
            agent_id,
            game_rules,
            world_building_rules,
            is_alive,
        };
        serde_json::to_string(&registered_msg).ok()
    };

    // 发送注册成功确认（在块外，避免跨 await 问题）
    if let Some(json) = registered_json {
        if tx.send(Message::Text(json.into())).await.is_err() {
            error!(
                "Failed to send Registered message to agent '{}'",
                agent_name
            );
        } else {
            debug!(
                "Sent Registered message with game rules to agent '{}' ({})",
                agent_name, agent_id
            );
        }
    }

    // ===== 连接后立即推送当前 WorldState =====
    // Agent 不需要等第一个 tick 就能看到自己的存活状态
    if agent_id != uuid::Uuid::nil() {
        match crate::db::get_latest_agent_state(&state.db_pool, agent_id).await {
            Ok(agent_state) => {
                // 计算 deadline：距离下次 tick 的剩余毫秒数
                let deadline_ms = {
                    let gd = state.game_data.get();
                    let tick_secs =
                        gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64;
                    drop(gd);
                    // 简化：给一个完整 tick 周期，因为 agent 刚连接
                    tick_secs * 1000
                };

                // 加载初始背包物品
                let initial_inventory =
                    match InventoryManager::get_all_items(&state.db_pool, agent_id).await {
                        Ok(items) => items
                            .into_iter()
                            .map(|item| {
                                let name = ItemRegistry::get(&item.item_id)
                                    .map(|config| config.name.clone())
                                    .unwrap_or_else(|| item.item_id.clone());
                                crate::models::InventoryItem {
                                    item_id: item.item_id,
                                    name,
                                    quantity: item.quantity,
                                    is_equipped: item.is_equipped,
                                }
                            })
                            .collect(),
                        Err(e) => {
                            warn!("加载 Agent {} 初始背包失败: {}", agent_id, e);
                            vec![]
                        }
                    };

                // 构建 WorldState（简化版，不含其他 agent entities）
                let world_state = crate::tick::build_initial_world_state(
                    &agent_state,
                    &state.game_data,
                    deadline_ms,
                    initial_inventory,
                );
                let ws_msg =
                    cyber_jianghu_protocol::ServerMessage::WorldState { data: world_state };
                if let Ok(ws_json) = serde_json::to_string(&ws_msg) {
                    if tx.send(Message::Text(ws_json.into())).await.is_err() {
                        warn!(
                            "Failed to send initial WorldState to agent '{}' ({})",
                            agent_name, agent_id
                        );
                    } else {
                        info!(
                            "Sent initial WorldState to agent '{}' (alive={})",
                            agent_name, agent_state.is_alive
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Failed to load agent state for initial WorldState: agent={}, err={}",
                    agent_id, e
                );
            }
        }
    }

    // 启动发送任务（从通道接收消息并发送到 WebSocket）
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // 心跳任务（主动发送 Ping 检测连接活性）
    let tx_for_heartbeat = tx.clone();
    let agent_name_for_heartbeat = agent_name.clone();
    let heartbeat_task = tokio::spawn(async move {
        // 每 30 秒发送一次 Ping
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            // 发送 Ping 消息
            if tx_for_heartbeat
                .send(Message::Ping(Bytes::new()))
                .await
                .is_err()
            {
                debug!(
                    "Heartbeat failed for agent '{}', connection likely closed",
                    agent_name_for_heartbeat
                );
                break;
            }
            debug!(
                "Sent heartbeat Ping to agent '{}'",
                agent_name_for_heartbeat
            );
        }
    });

    // 接收消息循环
    // Clone values for use in recv_task
    let state_for_recv = state.clone();
    let agent_name_for_recv = agent_name.clone();
    let device_id_for_recv = device_id;

    let recv_task = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(msg) => match msg {
                    Message::Text(text) => {
                        // 安全地截取文本预览（避免在 UTF-8 字符边界截断导致 panic）
                        let preview = if text.len() > 50 {
                            // 找到第 50 字节附近的字符边界
                            let end = text
                                .char_indices()
                                .take_while(|(idx, _)| *idx < 50)
                                .last()
                                .map(|(idx, c)| idx + c.len_utf8())
                                .unwrap_or(0);
                            &text[..end.min(text.len())]
                        } else {
                            &text
                        };
                        debug!(
                            "Received text message from agent '{}': len={}, preview={}",
                            agent_name_for_recv,
                            text.len(),
                            preview
                        );

                        // 解析消息
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(client_msg) => {
                                if let Err(e) = handle_client_message(
                                    &agent_id,
                                    device_id_for_recv,
                                    client_msg,
                                    &state_for_recv,
                                )
                                .await
                                {
                                    error!(
                                        "Failed to handle message from agent '{}': {}",
                                        agent_name_for_recv, e
                                    );

                                    // 发送错误消息给 Agent
                                    let error_msg = ServerMessage::Error {
                                        message: format!("Failed to process message: {}", e),
                                    };
                                    if let Ok(json) = serde_json::to_string(&error_msg) {
                                        let _ = tx.send(Message::Text(json.into())).await;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to parse message from agent '{}': {}",
                                    agent_name_for_recv, e
                                );

                                // 发送错误消息给 Agent
                                let error_msg = ServerMessage::Error {
                                    message: format!("Invalid message format: {}", e),
                                };
                                if let Ok(json) = serde_json::to_string(&error_msg) {
                                    let _ = tx.send(Message::Text(json.into())).await;
                                }
                            }
                        }
                    }
                    Message::Ping(data) => {
                        debug!("Received Ping from agent '{}'", agent_name_for_recv);
                        // 回复 Pong
                        let _ = tx.send(Message::Pong(data)).await;
                    }
                    Message::Pong(_) => {
                        debug!("Received Pong from agent '{}'", agent_name_for_recv);
                    }
                    Message::Close(_) => {
                        info!("Agent '{}' closed connection", agent_name_for_recv);
                        break;
                    }
                    _ => {
                        warn!(
                            "Received unsupported message type from agent '{}'",
                            agent_name_for_recv
                        );
                    }
                },
                Err(e) => {
                    warn!(
                        "WebSocket error from agent '{}': {}",
                        agent_name_for_recv, e
                    );
                    break;
                }
            }
        }
    });

    // 等待任一任务完成
    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
        _ = heartbeat_task => {},
    }

    // 清理连接
    {
        let mut connections = state.connection_manager.write().await;
        connections.remove(&device_id);
        info!(
            "Agent '{}' disconnected. Total online: {}",
            agent_name,
            connections.len()
        );
    }

    // 清理 agent_to_device_map（避免死亡通知发送到已断连设备）
    if agent_id != uuid::Uuid::nil() {
        let mut agent_to_device = state.agent_to_device_map.write().await;
        agent_to_device.remove(&agent_id);
    }

    info!("WebSocket handler finished for agent '{}'", agent_name);
}

// ============================================================================
// 消息处理
// ============================================================================

/// 处理客户端消息
///
/// 根据消息类型进行相应的处理
async fn handle_client_message(
    agent_id: &uuid::Uuid,
    device_id: uuid::Uuid,
    msg: ClientMessage,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match msg {
        ClientMessage::Intent {
            intent_id,
            tick_id,
            agent_id: msg_agent_id,
            thought_log,
            action_type,
            action_data,
            priority,
        } => {
            handle_intent(
                *agent_id,
                device_id,
                msg_agent_id,
                intent_id,
                tick_id,
                thought_log,
                action_type,
                action_data,
                priority,
                state,
            )
            .await
        }
        ClientMessage::Dialogue { message } => {
            handle_dialogue_message(*agent_id, message, state).await
        }
    }
}

/// 处理意图上报
///
/// 将 Intent 保存到 IntentManager（临时缓存）
/// 包含速率限制检查、Agent 存活检查和 tick_id 校验
#[allow(clippy::too_many_arguments)]
async fn handle_intent(
    connection_agent_id: uuid::Uuid,
    device_id: uuid::Uuid,
    msg_agent_id: Option<uuid::Uuid>,
    req_intent_id: Option<uuid::Uuid>,
    tick_id: i64,
    thought_log: Option<String>,
    action_type: String,
    action_data: Option<serde_json::Value>,
    priority: i32,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 确定最终的 agent_id
    // 如果客户端指定了 agent_id，验证其属于该 device
    let agent_id = match msg_agent_id {
        Some(id) => {
            // 使用 query_scalar 只查询 device_id，避免 SELECT *
            let owner_device_id: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT device_id FROM agents WHERE agent_id = $1")
                    .bind(id)
                    .fetch_optional(&state.db_pool)
                    .await
                    .context("查询 Agent 归属失败")?;

            match owner_device_id {
                Some(owner) if owner == device_id => {
                    tracing::debug!("Agent {} ownership verified for device {}", id, device_id);
                    id
                }
                Some(_) => {
                    tracing::warn!(
                        "Agent ownership mismatch: agent={}, device={}",
                        id,
                        device_id
                    );
                    return Err("无权操作此角色".into());
                }
                None => {
                    return Err("Agent 不存在".into());
                }
            }
        }
        None => connection_agent_id,
    };

    // 速率限制检查
    if !crate::state::check_rate_limit(&state.rate_limiter, agent_id).await {
        warn!("Rate limit exceeded for agent {}", agent_id);
        return Err("Rate limit exceeded. Please wait before sending another intent.".into());
    }

    // Agent 存活检查：死亡的 Agent 不允许提交意图
    let agent_state = crate::db::get_latest_agent_state(&state.db_pool, agent_id).await?;
    if !agent_state.is_alive {
        warn!("Intent rejected: agent {} is dead", agent_id);
        return Err("Agent 已死亡，无法执行此动作。请重新转生入世。".into());
    }

    // 纵深防御：检查 agents.status，拒绝已归隐/已死亡角色的意图
    let agent_status: Option<String> =
        sqlx::query_scalar("SELECT status FROM agents WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_optional(&state.db_pool)
            .await
            .context("查询 Agent 状态失败")?
            .flatten();

    if agent_status.as_deref() != Some("active") {
        warn!(
            "Intent rejected: agent {} status is {:?}, expected 'active'",
            agent_id, agent_status
        );
        return Err("角色已失效，无法执行此动作。请重新转生入世。".into());
    }

    // tick_id 校验：只接受当前 tick 的意图（硬性要求）
    let current_tick = crate::db::get_current_world_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    if tick_id != current_tick {
        warn!(
            "Intent tick_id mismatch: agent={}, intent_tick={}, current_tick={}",
            agent_id, tick_id, current_tick
        );
        return Err(format!(
            "Intent tick_id {} 不匹配当前 tick {}。请提交当前 tick 的意图。",
            tick_id, current_tick
        )
        .into());
    }

    info!(
        "Intent received from agent {}: tick={}, action={}",
        agent_id, tick_id, action_type
    );

    // 解析动作类型（数据驱动：直接使用字符串）
    let action = crate::models::ActionType::new(&action_type);

    // 构造 Intent
    let intent = Intent {
        intent_id: req_intent_id.unwrap_or_else(uuid::Uuid::new_v4), // 如果 ClientMessage 中没有传 intent_id，这里生成一个新的
        agent_id,
        tick_id,
        thought_log,
        action_type: action,
        action_data,
        priority,
        observer_thought: None,
        narrative: None,
    };

    // 保存到 IntentManager（临时缓存）
    {
        let mut intents = state.intent_manager.write().await;
        intents.insert(agent_id, intent);
    }

    info!(
        "Intent saved to cache for agent {} in tick {}",
        agent_id, tick_id
    );

    Ok(())
}

/// 处理对话消息
///
/// 将对话消息转发给对话管理器，并根据响应路由到相应的 Agent
async fn handle_dialogue_message(
    agent_id: uuid::Uuid,
    message: DialogueMessage,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("对话消息来自 agent {}: {:?}", agent_id, message);

    // 调用对话管理器处理消息
    let response = state.dialogue_manager.handle_message(message.clone()).await;

    match response {
        Ok(dialogue_response) => {
            match dialogue_response {
                DialogueResponse::RequestForwarded {
                    session_id,
                    target_agent_id,
                } => {
                    // 转发请求给目标 Agent
                    let forward_msg = DialogueMessage::Request {
                        from_agent_id: agent_id,
                        to_agent_id: target_agent_id,
                        opening_remark: match &message {
                            DialogueMessage::Request { opening_remark, .. } => {
                                opening_remark.clone()
                            }
                            _ => String::new(),
                        },
                    };
                    super::broadcast::forward_dialogue_message(
                        target_agent_id,
                        forward_msg,
                        &state.connection_manager,
                    )
                    .await?;
                    debug!(
                        "对话请求已转发: session={}, to={}",
                        session_id, target_agent_id
                    );
                }
                DialogueResponse::SessionStarted {
                    session_id,
                    agent_a,
                    agent_b,
                } => {
                    // 通知双方会话已建立
                    let started_msg = DialogueMessage::Accept {
                        session_id: session_id.clone(),
                        from_agent_id: agent_id,
                    };
                    super::broadcast::forward_dialogue_message(
                        agent_a,
                        started_msg.clone(),
                        &state.connection_manager,
                    )
                    .await?;
                    super::broadcast::forward_dialogue_message(
                        agent_b,
                        started_msg,
                        &state.connection_manager,
                    )
                    .await?;
                    debug!("会话已建立，双方已通知: session={}", session_id);
                }
                DialogueResponse::SessionRejected {
                    session_id,
                    rejected_by,
                    requester,
                } => {
                    // 通知请求发起者被拒绝
                    let rejected_msg = DialogueMessage::Reject {
                        session_id: session_id.clone(),
                        from_agent_id: rejected_by,
                        reason: None,
                    };
                    super::broadcast::forward_dialogue_message(
                        requester,
                        rejected_msg,
                        &state.connection_manager,
                    )
                    .await?;
                    debug!(
                        "会话已拒绝: session={}, rejected_by={}, notified={}",
                        session_id, rejected_by, requester
                    );
                }
                DialogueResponse::ContentForward {
                    session_id,
                    from_agent_id,
                    to_agent_id,
                } => {
                    // 转发内容给目标 Agent
                    let content_msg = match &message {
                        DialogueMessage::Content {
                            session_id,
                            from_agent_id,
                            content,
                        } => DialogueMessage::Content {
                            session_id: session_id.clone(),
                            from_agent_id: *from_agent_id,
                            content: content.clone(),
                        },
                        _ => return Err("Invalid message type for ContentForward".into()),
                    };
                    super::broadcast::forward_dialogue_message(
                        to_agent_id,
                        content_msg,
                        &state.connection_manager,
                    )
                    .await?;
                    debug!(
                        "对话内容已转发: session={}, from={}, to={}",
                        session_id, from_agent_id, to_agent_id
                    );
                }
                DialogueResponse::SessionEnded {
                    session_id,
                    ended_by,
                    other_participant,
                } => {
                    // 通知另一方会话已结束
                    let end_msg = DialogueMessage::End {
                        session_id: session_id.clone(),
                        from_agent_id: ended_by,
                    };
                    super::broadcast::forward_dialogue_message(
                        other_participant,
                        end_msg,
                        &state.connection_manager,
                    )
                    .await?;
                    debug!(
                        "会话已结束: session={}, ended_by={}, notified={}",
                        session_id, ended_by, other_participant
                    );
                }
            }
        }
        Err(e) => {
            warn!("对话消息处理失败: {}", e);
            // 发送错误消息给发起者
            let error_msg = ServerMessage::Error {
                message: format!("Dialogue failed: {}", e),
            };
            let json = serde_json::to_string(&error_msg)?;
            let connections = state.connection_manager.read().await;
            if let Some(connection) = connections.get(&agent_id) {
                let _ = connection.send(Message::Text(json.into())).await;
            }
        }
    }

    Ok(())
}
