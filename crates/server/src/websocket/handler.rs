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
use sha2::Digest as _;
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
    let ws_config = crate::game_data::NetworkRegistry::websocket();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(ws_config.channel_capacity);

    // 心跳追踪：连续未收到 Pong 的次数
    let pings_without_pong = Arc::new(AtomicU8::new(0));
    let max_missed_pongs = ws_config.max_missed_pongs;
    let heartbeat_interval = ws_config.heartbeat_interval_secs;
    let log_preview_length = ws_config.log_preview_length;

    // 添加到连接管理器（使用 device_id 作为 key）
    // 重连时：先移除旧连接，确保旧 send_task 收到通道关闭信号并退出
    let my_connection_id = {
        let mut connections = state.connection_manager.write().await;
        // 如果该 device_id 已有连接，先移除（触发旧 send_task 退出）
        connections.remove(&device_id);
        let connection = Connection::new(agent_id, device_id, agent_name.clone(), tx.clone());
        let conn_id = connection.connection_id;
        connections.insert(device_id, connection);
        info!(
            "Agent '{}' added to online list (device={}, conn={}). Total online: {}",
            agent_name,
            device_id,
            conn_id,
            connections.len()
        );
        conn_id
    };

    // 更新 agent_id → device_id 反向映射（用于 WebSocket 广播）
    // 重要：WebSocket 重连时需要更新映射，因为 agent_register 只在首次注册时调用
    if agent_id != uuid::Uuid::nil() {
        let mut agent_to_device = state.agent_to_device_map.write().await;
        rebind_device_agent(&mut agent_to_device, agent_id, device_id);
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
        let rebirth_delay_ticks = gd.game_rules.data.agent_state.survival.rebirth.delay_ticks;
        let rebirth_retry_max_attempts = gd
            .game_rules
            .data
            .agent_state
            .survival
            .rebirth
            .retry_max_attempts;
        let rebirth_retry_interval_secs = gd
            .game_rules
            .data
            .agent_state
            .survival
            .rebirth
            .retry_interval_secs;
        let game_rules_version = gd.game_rules.version.clone();
        let immediate_events = gd.game_rules.data.immediate_events.clone();
        let intent_batch = gd.game_rules.data.intent_batch.clone();
        let dialogue_context = gd.game_rules.data.dialogue_context.clone();
        drop(gd);

        let survival = super::types::SurvivalConfig {
            rebirth_delay_ticks,
            rebirth_retry_max_attempts,
            rebirth_retry_interval_secs,
        };
        let game_rules = build_game_rules_from_config(
            tick_duration_secs,
            survival,
            game_rules_version,
            immediate_events,
            intent_batch,
            dialogue_context,
        );

        // 加载世界观规则（可选）
        let world_building_rules = load_world_building_rules();

        // 加载叙事化配置
        let (narrative_config, narrative_config_hash) = {
            let gd = state.game_data.get();
            let nc = gd.narrative.clone();
            let hash = serde_json::to_vec(&nc)
                .ok()
                .map(|bytes| format!("{:x}", sha2::Sha256::digest(&bytes)));
            (Some(nc), hash)
        };

        let registered_msg = ServerMessage::Registered {
            agent_id,
            game_rules,
            world_building_rules,
            is_alive,
            agent_name: if agent_name != "Pending" {
                Some(agent_name.clone())
            } else {
                None
            },
            narrative_config,
            narrative_config_hash,
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

    // game_rules 和 world_building_rules 已通过 Registered 消息下发，无需重复发送 ConfigUpdate
    // 热更新路径（admin reload-config）仍通过 broadcast_config_update 触发

    // ===== 发送技能配置（ConfigUpdate） =====
    // Agent 连接后仅下发该 Agent 已掌握的技能内容
    if agent_id != uuid::Uuid::nil() {
        // 优先从 DashMap 读取（已在内存中）
        let agent_skills: Vec<String> = match state.agent_state_cache.get(&agent_id) {
            Some(r) => r.value().skills.clone(),
            None => {
                // Fallback: DashMap miss 时查 DB（首连场景）
                crate::db::get_latest_agent_state(&state.db_pool, agent_id)
                    .await
                    .map(|s| s.skills.clone())
                    .unwrap_or_default()
            }
        };

        let all_skills = crate::game_data::registry::SkillRegistry::all_with_id();
        let skill_contents: Vec<cyber_jianghu_protocol::types::SkillContent> = all_skills
            .into_iter()
            .filter(|s| agent_skills.contains(&s.skill_id))
            .map(|s| cyber_jianghu_protocol::types::SkillContent {
                skill_id: s.skill_id,
                name: s.definition.name,
                body: s.definition.content,
            })
            .collect();

        if !skill_contents.is_empty() {
            let config_update = ServerMessage::ConfigUpdate {
                config_type: "skills".to_string(),
                update_type: "full".to_string(),
                version: "1.0.0".to_string(),
                content: serde_json::to_value(&skill_contents).unwrap_or_default(),
                content_hash: None,
                updated_items: vec![],
                removed_items: vec![],
            };

            if let Err(e) = broadcast::send_config_update(
                agent_id,
                config_update,
                &state.connection_manager,
                &state.agent_to_device_map,
            )
            .await
            {
                warn!(
                    "Failed to send skills ConfigUpdate to agent {}: {}",
                    agent_id, e
                );
            } else {
                debug!(
                    "Sent {} skills ConfigUpdate to agent '{}' ({})",
                    skill_contents.len(),
                    agent_name,
                    agent_id
                );
            }
        }
    }

    // ===== 发送 prompt_templates（ConfigUpdate，JSON 格式） =====
    if agent_id != uuid::Uuid::nil() {
        let cache = state.prompt_template_cache.read().await;
        if let Some(ref pt_cache) = *cache {
            let config_update = ServerMessage::ConfigUpdate {
                config_type: "prompt_templates".to_string(),
                update_type: "full".to_string(),
                version: pt_cache.version.clone(),
                content: pt_cache.json_value.clone(),
                content_hash: Some(pt_cache.hash.clone()),
                updated_items: vec![],
                removed_items: vec![],
            };

            if let Err(e) = broadcast::send_config_update(
                agent_id,
                config_update,
                &state.connection_manager,
                &state.agent_to_device_map,
            )
            .await
            {
                warn!(
                    "Failed to send prompt_templates ConfigUpdate to agent {}: {}",
                    agent_id, e
                );
            } else {
                debug!(
                    "Sent prompt_templates ConfigUpdate to agent '{}' ({})",
                    agent_name, agent_id
                );
            }
        }
    }

    // ===== 发送 persona_event_rules（ConfigUpdate，JSON 格式） =====
    if agent_id != uuid::Uuid::nil() {
        let rules_path = crate::paths::get_config_dir().join("persona_event_rules.yaml");
        match std::fs::read_to_string(&rules_path) {
            Ok(yaml_content) => match serde_yaml::from_str::<serde_json::Value>(&yaml_content) {
                Ok(json_value) => {
                    let config_update = ServerMessage::ConfigUpdate {
                        config_type: "persona_event_rules".to_string(),
                        update_type: "full".to_string(),
                        version: "1.0".to_string(),
                        content: json_value,
                        content_hash: None,
                        updated_items: vec![],
                        removed_items: vec![],
                    };

                    if let Err(e) = broadcast::send_config_update(
                        agent_id,
                        config_update,
                        &state.connection_manager,
                        &state.agent_to_device_map,
                    )
                    .await
                    {
                        warn!(
                            "Failed to send persona_event_rules ConfigUpdate to agent {}: {}",
                            agent_id, e
                        );
                    } else {
                        debug!(
                            "Sent persona_event_rules ConfigUpdate to agent '{}' ({})",
                            agent_name, agent_id
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to parse persona_event_rules.yaml as JSON for agent {}: {}",
                        agent_id, e
                    );
                }
            },
            Err(e) => {
                warn!(
                    "Failed to read persona_event_rules.yaml for agent {}: {}",
                    agent_id, e
                );
            }
        }
    }

    // ===== 发送 narrative_config（ConfigUpdate，JSON 格式） =====
    if agent_id != uuid::Uuid::nil() {
        let (nc, nc_hash) = {
            let gd = state.game_data.get();
            let narrative = gd.narrative.clone();
            let hash = serde_json::to_vec(&narrative)
                .ok()
                .map(|bytes| format!("{:x}", sha2::Sha256::digest(&bytes)));
            (narrative, hash)
        };
        let config_update = ServerMessage::ConfigUpdate {
            config_type: "narrative_config".to_string(),
            update_type: "full".to_string(),
            version: "1.0".to_string(),
            content: serde_json::to_value(&nc).unwrap_or_default(),
            content_hash: nc_hash,
            updated_items: vec![],
            removed_items: vec![],
        };

        if let Err(e) = broadcast::send_config_update(
            agent_id,
            config_update,
            &state.connection_manager,
            &state.agent_to_device_map,
        )
        .await
        {
            warn!(
                "Failed to send narrative_config ConfigUpdate to agent {}: {}",
                agent_id, e
            );
        } else {
            debug!(
                "Sent narrative_config ConfigUpdate to agent '{}' ({})",
                agent_name, agent_id
            );
        }
    }

    // ===== 连接后立即推送当前 WorldState =====
    // Agent 不需要等第一个 tick 就能看到自己的存活状态
    if agent_id != uuid::Uuid::nil() {
        match crate::db::get_latest_agent_state(&state.db_pool, agent_id).await {
            Ok(agent_state) => {
                // 将 agent 状态加入 DashMap（实时模式：广播从 DashMap 读取 agent 列表）
                if agent_state.is_alive {
                    let current_tick = state
                        .current_accepting_tick_id
                        .load(std::sync::atomic::Ordering::Acquire);
                    let mut state_for_cache = agent_state.clone();
                    state_for_cache.tick_id = current_tick;
                    state.agent_state_cache.insert(agent_id, state_for_cache);
                    info!(
                        "Agent '{}' ({}) loaded into DashMap (tick={})",
                        agent_name, agent_id, current_tick
                    );
                }

                // 加载初始背包物品：失败时直接关闭连接，不构造假 WorldState
                let initial_inventory = match load_initial_inventory(&state.db_pool, agent_id).await
                {
                    Ok(items) => items,
                    Err(e) => {
                        error!(
                            "加载 Agent {} 初始背包失败，关闭 WebSocket: {:#}",
                            agent_id, e
                        );
                        let _ = tx
                            .send(Message::Close(Some(CloseFrame {
                                code: 1011,
                                reason: axum::extract::ws::Utf8Bytes::from_static(
                                    "initial_inventory_load_failed",
                                ),
                            })))
                            .await;
                        return;
                    }
                };

                // 加载当前节点地面物品
                let nearby_items =
                    match load_nearby_ground_items(&state.db_pool, &agent_state.node_id).await {
                        Ok(items) => items,
                        Err(e) => {
                            error!(
                                "加载 Agent {} 节点地面物品失败，关闭 WebSocket: {:#}",
                                agent_id, e
                            );
                            let _ = tx
                                .send(Message::Close(Some(CloseFrame {
                                    code: 1011,
                                    reason: axum::extract::ws::Utf8Bytes::from_static(
                                        "nearby_ground_items_load_failed",
                                    ),
                                })))
                                .await;
                            return;
                        }
                    };

                // 构建 WorldState（简化版，不含其他 agent entities）
                // 重连时使用当前 tick_id 而非 agent_state.tick_id，避免 TickMismatch
                let current_tick = state
                    .current_accepting_tick_id
                    .load(std::sync::atomic::Ordering::Acquire);
                let gd = state.game_data.snapshot();
                let loc = state.game_data.location_snapshot();
                let recipe_ids = crate::db::get_known_recipe_ids(&state.db_pool, agent_id)
                    .await
                    .unwrap_or_default();
                let recipe_details = crate::tick::build_recipe_details(&recipe_ids);
                let world_state = crate::tick::build_initial_world_state(
                    &agent_state,
                    &gd,
                    &loc,
                    initial_inventory,
                    nearby_items,
                    Some(current_tick),
                    recipe_details,
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
    let pings_without_pong_for_heartbeat = pings_without_pong.clone();
    let heartbeat_task = tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(tokio::time::Duration::from_secs(heartbeat_interval));
        loop {
            interval.tick().await;
            pings_without_pong_for_heartbeat.fetch_add(1, Ordering::Relaxed);
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
            if pings_without_pong_for_heartbeat.load(Ordering::Relaxed) >= max_missed_pongs {
                warn!(
                    "Agent '{}' missed {} pongs, closing connection",
                    agent_name_for_heartbeat, max_missed_pongs
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
    let state_for_recv = state.clone();
    let agent_name_for_recv = agent_name.clone();
    let device_id_for_recv = device_id;
    let pings_without_pong_for_recv = pings_without_pong.clone();

    let recv_task = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(msg) => match msg {
                    Message::Text(text) => {
                        // 安全地截取文本预览（避免在 UTF-8 字符边界截断导致 panic）
                        let preview = if text.len() > log_preview_length {
                            // 找到截断字节附近的字符边界
                            let end = text
                                .char_indices()
                                .take_while(|(idx, _)| *idx < log_preview_length)
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

                                    // 发送错误消息给 Agent（尝试提取结构化错误码）
                                    let (code, message, current_tick_id) =
                                        if let Some(ge) = e.downcast_ref::<GameError>() {
                                            (
                                                ge.error_code().to_string(),
                                                ge.to_string(),
                                                ge.current_tick_id(),
                                            )
                                        } else {
                                            (
                                                cyber_jianghu_protocol::ERROR_CODE_ACTION_FAILED
                                                    .to_string(),
                                                format!("Failed to process message: {}", e),
                                                None,
                                            )
                                        };
                                    let error_msg = ServerMessage::Error {
                                        code,
                                        message,
                                        current_tick_id,
                                    };
                                    if let Ok(json) = serde_json::to_string(&error_msg)
                                        && let Err(e) = tx.send(Message::Text(json.into())).await
                                    {
                                        tracing::warn!(
                                            "ws error_msg.send 失败（receiver 可能已 drop）：{e:?}"
                                        );
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
                                    code: cyber_jianghu_protocol::ERROR_CODE_INVALID_MESSAGE
                                        .to_string(),
                                    message: format!("Invalid message format: {}", e),
                                    current_tick_id: None,
                                };
                                if let Ok(json) = serde_json::to_string(&error_msg)
                                    && let Err(e) = tx.send(Message::Text(json.into())).await
                                {
                                    tracing::warn!(
                                        "ws error_msg.send（site 2）失败（receiver 可能已 drop）：{e:?}"
                                    );
                                }
                            }
                        }
                    }
                    Message::Ping(data) => {
                        debug!("Received Ping from agent '{}'", agent_name_for_recv);
                        // 回复 Pong
                        if let Err(e) = tx.send(Message::Pong(data)).await {
                            tracing::warn!("ws Pong.send 失败（receiver 可能已 drop）：{e:?}");
                        }
                    }
                    Message::Pong(_) => {
                        pings_without_pong_for_recv.store(0, Ordering::Relaxed);
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

    // 清理连接：仅删除自己创建的连接（新连接可能已接管同一 device_id）
    {
        let mut connections = state.connection_manager.write().await;
        let should_remove = connections
            .get(&device_id)
            .map(|c| c.connection_id == my_connection_id)
            .unwrap_or(false);
        if should_remove {
            connections.remove(&device_id);
            info!(
                "Agent '{}' disconnected (conn={}). Total online: {}",
                agent_name,
                my_connection_id,
                connections.len()
            );
        } else {
            info!(
                "Agent '{}' handler exiting, new connection already took over (device={}). Total online: {}",
                agent_name,
                device_id,
                connections.len()
            );
        }
    }

    // 清理 agent_to_device_map：仅在没有活跃连接时删除
    if agent_id != uuid::Uuid::nil() {
        let has_active_connection = {
            let connections = state.connection_manager.read().await;
            connections.get(&device_id).is_some()
        };
        if !has_active_connection {
            let mut agent_to_device = state.agent_to_device_map.write().await;
            agent_to_device.remove(&agent_id);
        }
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
            subsequent_intents,
            soul_cycle_metadata,
            chaos_marker,
            dream_marker,
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
                subsequent_intents,
                soul_cycle_metadata,
                chaos_marker,
                dream_marker,
                state,
            )
            .await
        }
        ClientMessage::Dialogue { message } => {
            handle_dialogue_message(*agent_id, message, state).await
        }
        ClientMessage::SoulCycleReport {
            tick_id,
            agent_id: msg_agent_id,
            pipe_seq,
            metadata,
        } => {
            handle_soul_cycle_report(device_id, msg_agent_id, tick_id, pipe_seq, &metadata, state)
                .await
        }
        ClientMessage::DailySummary { game_day, summary } => {
            handle_daily_summary(device_id, game_day, &summary, state).await
        }
        ClientMessage::TraceReport { traces } => {
            handle_trace_report(device_id, &traces, state).await
        }
    }
}

/// 处理训练 Trace 上报（agent → server 汇聚）
///
/// agent 端的结构化 LLM 调用 trace（已脱敏）批量回传，server 落盘后
/// 与 reward 同目录树（get_data_dir()），训练导出时按 (agent_id, tick_id) join。
async fn handle_trace_report(
    device_id: uuid::Uuid,
    traces: &[cyber_jianghu_protocol::TraceEntry],
    state: &std::sync::Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 通过 device_id 查找当前 agent（对齐 handle_daily_summary 模式）
    let agent_id = match crate::db::get_agent_by_device_id(&state.db_pool, device_id).await {
        Ok(Some(agent)) => agent.agent_id,
        Ok(None) => return Err("无关联角色".into()),
        Err(e) => return Err(format!("查询角色失败: {}", e).into()),
    };

    // 落盘到 server 侧 traces/（与 rewards/ 同根目录）
    let traces_dir = crate::paths::get_data_dir().join("traces");
    for entry in traces {
        let soul = &entry.soul_stage;
        // date 用 trace 真实产生时间（wall_clock 前10位=YYYY-MM-DD），非 server 接收时间
        let date = entry
            .wall_clock
            .as_deref()
            .and_then(|wc| wc.get(..10).map(|s| s.to_string()))
            .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
        let dir = traces_dir
            .join(format!("soul={}", soul))
            .join(format!("agent={}", agent_id));
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            tracing::error!("[trace] server 创建目录失败: {}", e);
            continue;
        }
        let path = dir.join(format!("date={}.jsonl", date));
        let line = match serde_json::to_string(entry) {
            Ok(s) => s + "\n",
            Err(e) => {
                tracing::warn!("[trace] server 序列化失败: {}", e);
                continue;
            }
        };
        use tokio::io::AsyncWriteExt;
        match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
        {
            Ok(mut f) => {
                if let Err(e) = f.write_all(line.as_bytes()).await {
                    tracing::warn!("[trace] server 写入失败 {:?}: {}", path, e);
                }
            }
            Err(e) => tracing::warn!("[trace] server 打开文件失败 {:?}: {}", path, e),
        }
    }

    tracing::debug!(
        "[trace] server 收到 {} 条 trace（device={}, agent={}）",
        traces.len(),
        device_id,
        agent_id
    );
    Ok(())
}

/// 处理意图上报
///
/// 处理 Agent 提交的 Intent（实时模式：非阻塞入队 IntentWorker）
/// 包含速率限制检查、Agent 存活检查，speak/whisper 即时广播
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
    subsequent_intents: Vec<Intent>,
    soul_cycle_metadata: Option<cyber_jianghu_protocol::SoulCycleMetadata>,
    chaos_marker: Option<cyber_jianghu_protocol::types::ChaosMarker>,
    dream_marker: Option<cyber_jianghu_protocol::types::DreamMarker>,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 确定最终的 agent_id
    // 如果客户端指定了 agent_id，验证其属于该 device
    let agent_id = match msg_agent_id {
        Some(id) if id != uuid::Uuid::nil() => {
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
        // nil / None → 使用连接绑定的 agent_id（即时 intent 由 WebSocket 后台填充）
        _ => connection_agent_id,
    };

    let intent_id = req_intent_id.unwrap_or_else(uuid::Uuid::new_v4);

    // Handler 层拒绝时发送 ExecutionResult 给 agent（让 OutcomeMemory 记录失败）
    let reject_and_notify = |err_msg: String| async {
        let governance_code = ServerGovernanceMapper::map_from_error(&err_msg);
        let msg = cyber_jianghu_protocol::ServerMessage::ExecutionResult {
            tick_id,
            intent_id,
            success: false,
            error: Some(err_msg.clone()),
            state_change_summary: None,
            governance_code: Some(governance_code),
        };
        let _ = crate::tick::send_to_agent(
            agent_id,
            &msg,
            &state.connection_manager,
            &state.agent_to_device_map,
        )
        .await;
        Err::<(), Box<dyn std::error::Error + Send + Sync>>(err_msg.into())
    };

    // 速率限制检查
    if !crate::state::check_rate_limit(&state.rate_limiter, agent_id).await {
        warn!("Rate limit exceeded for agent {}", agent_id);
        return reject_and_notify(
            "Rate limit exceeded. Please wait before sending another intent.".into(),
        )
        .await;
    }

    // Agent 存活检查：从 DashMap 内存缓存读取（实时模式，不再查 DB）
    // 防御性：如果 DashMap 缺失 entry（auto-rebirth 后 WS 重连竞态），从 DB 回补
    let is_alive = state
        .agent_state_cache
        .get(&agent_id)
        .map(|r| r.value().is_alive)
        .unwrap_or_else(|| {
            // DashMap 缺失，尝试从 DB 加载（防御 auto-rebirth race condition）
            warn!("Agent {} not in DashMap, attempting DB fallback", agent_id);
            false
        });

    // 如果 DashMap 没有命中，尝试从 DB 防御性加载
    if !is_alive
        && !state.agent_state_cache.contains_key(&agent_id)
        && let Ok(db_state) = crate::db::get_latest_agent_state(&state.db_pool, agent_id).await
        && db_state.is_alive
    {
        let current_tick = state
            .current_accepting_tick_id
            .load(std::sync::atomic::Ordering::Acquire);
        // 保留 DB 原始 tick_id，让 decay 引擎根据差值补算衰减
        state.agent_state_cache.insert(agent_id, db_state.clone());
        info!(
            "DashMap miss → DB defensive load: agent {} (db_tick={}, current_tick={})",
            agent_id, db_state.tick_id, current_tick
        );
    }

    // 二次检查：防御性加载后重新读 DashMap
    let is_alive = if !is_alive {
        state
            .agent_state_cache
            .get(&agent_id)
            .map(|r| r.value().is_alive)
            .unwrap_or(false)
    } else {
        true
    };

    if !is_alive {
        warn!(
            "Intent rejected: agent {} is dead or not in cache",
            agent_id
        );
        return reject_and_notify(format!("Agent {} is dead or not in cache", agent_id)).await;
    }

    info!(
        "Intent received from agent {}: tick={}, action={}",
        agent_id, tick_id, action_type
    );

    // 快捷代理：吃/喝 → 用（允许 LLM 以更自然的语言表达消耗意图）
    // 归一化后再赋回 action_type，后续的 ActionRegistry 查询和 executor 分发
    // 都使用归一化后的动作类型。原始值已记入日志供调试。
    let action_type = match action_type.as_str() {
        "吃" | "喝" => "用".to_string(),
        other => other.to_string(),
    };

    // 递归归一化 subsequent_intents 中的动作类型
    let mut subsequent_intents = subsequent_intents;
    for sub in &mut subsequent_intents {
        let normalized = match sub.action_type.as_str() {
            "吃" | "喝" => "用".to_string(),
            x => x.to_string(),
        };
        sub.action_type = crate::models::ActionType::new(normalized);
    }

    // 解析动作类型（数据驱动：直接使用字符串）
    let action = crate::models::ActionType::new(&action_type);

    // 验证 subsequent_intents 安全性
    let max_subsequent = crate::game_data::registry()
        .map(|c| {
            c.get()
                .game_rules
                .data
                .intent_batch
                .as_ref()
                .map(|ib| ib.max_intents_per_tick)
                .unwrap_or(3)
        })
        .unwrap_or(3)
        .saturating_sub(1); // 减去 primary intent 自身

    if subsequent_intents.len() > max_subsequent {
        warn!(
            "Pipeline 过长: agent={} 有 {} 个 subsequent intents，上限 {}",
            agent_id,
            subsequent_intents.len(),
            max_subsequent
        );
        return reject_and_notify(format!("Pipeline 过长: 最多 {} 个后续动作", max_subsequent))
            .await;
    }

    // 递归拒绝：只允许单层 pipeline
    for (i, sub) in subsequent_intents.iter().enumerate() {
        if !sub.subsequent_intents.is_empty() {
            warn!(
                "嵌套 pipeline 拒绝: agent={} subsequent[{}] 含嵌套 intents",
                agent_id, i
            );
            return reject_and_notify(
                "不支持嵌套 pipeline，subsequent intents 不可再包含 subsequent".into(),
            )
            .await;
        }
    }

    // agent_id 一致性验证
    for (i, sub) in subsequent_intents.iter().enumerate() {
        if sub.agent_id != uuid::Uuid::nil() && sub.agent_id != agent_id {
            warn!(
                "agent_id 不一致: agent={} subsequent[{}] agent_id={}",
                agent_id, i, sub.agent_id
            );
            return reject_and_notify(format!("subsequent intent[{}] agent_id 不一致", i)).await;
        }
    }

    // 构造 Intent（chaos_marker/dream_marker 从 ClientMessage 透传，不再硬编码丢弃）
    let mut intent = Intent {
        intent_id: req_intent_id.unwrap_or_else(uuid::Uuid::new_v4),
        agent_id,
        tick_id,
        thought_log,
        action_type: action,
        action_data: action_data.clone(),
        priority,
        reflector_thought: None,
        chaos_marker,
        dream_marker,
        already_broadcast: false,
        session_id: None,
        subsequent_intents,
    };

    let transmission = ActionRegistry::get(action_type.as_str())
        .map(|c| c.transmission)
        .unwrap_or_default();

    // Broadcast: 公共频道广播给同 Location 的所有在线 Agent
    if transmission == Transmission::Broadcast
        && let Some(content_value) = action_data.as_ref().and_then(|d| d.get("content"))
        && let Some(content_str) = content_value.as_str()
    {
        let location = state
            .agent_state_cache
            .get(&agent_id)
            .map(|r| r.value().node_id.clone())
            .ok_or_else(|| anyhow::anyhow!("Agent {} 不在缓存中", agent_id))?;

        // 独立任务：广播，避免阻塞 intent 处理主流程
        let state_clone = state.clone();
        let content_owned = content_str.to_string();
        let agent_id_for_log = agent_id;
        let intent_id_for_log = intent.intent_id;
        tokio::spawn(async move {
            if let Err(e) = super::broadcast::broadcast_speak_to_location(
                agent_id_for_log,
                &location,
                &content_owned,
                tick_id,
                &state_clone,
            )
            .await
            {
                tracing::warn!(
                    "Failed to broadcast speak intent immediately: agent={}, intent={}, error={}",
                    agent_id_for_log,
                    intent_id_for_log,
                    e
                );
            } else {
                tracing::debug!(
                    "Speak intent broadcast immediately to location {} for agent {}",
                    location,
                    agent_id_for_log
                );
            }
        });

        // 标记已广播
        intent.already_broadcast = true;
    }

    // Session: 定向 + 创建 Dialogue Session
    if transmission == Transmission::Session
        && let Some(target_value) = action_data.as_ref().and_then(|d| d.get("target_agent_id"))
        && let Some(target_id_str) = target_value.as_str()
    {
        let candidates: Vec<uuid::Uuid> = state
            .agent_state_cache
            .iter()
            .map(|r| r.value().agent_id)
            .collect();
        let target_agent_id =
            match cyber_jianghu_protocol::resolve_agent_id(target_id_str, &candidates) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!("无法解析 target_agent_id: {} ({})", target_id_str, e);
                    return reject_and_notify(format!("无效的 target_agent_id: {}", e)).await;
                }
            };

        match state
            .dialogue_manager
            .create_session(agent_id, target_agent_id)
            .await
        {
            Ok(response) => {
                if let DialogueResponse::RequestForwarded { session_id, .. } = response {
                    intent.session_id = Some(session_id.clone());
                    tracing::debug!(
                        "Whisper intent created Dialogue Session {} for agent {}",
                        session_id,
                        agent_id
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to create Dialogue Session for whisper: agent={}, target={}, error={}",
                    agent_id,
                    target_agent_id,
                    e
                );
            }
        }
    }

    // 路由到 IntentWorker（非阻塞 try_send，队列满时返回错误）
    // 提取 subsequent 信息（intent 即将被 move 进 WorkerMessage）
    let subsequent_summaries: Vec<(String, Option<serde_json::Value>, Option<cyber_jianghu_protocol::types::ChaosMarker>, Option<cyber_jianghu_protocol::types::DreamMarker>)> = intent
        .subsequent_intents
        .iter()
        .map(|si| {
            (
                si.action_type.to_string(),
                si.action_data.clone(),
                si.chaos_marker.clone(),
                si.dream_marker.clone(),
            )
        })
        .collect();

    match state
        .worker_tx
        .try_send(crate::tick::WorkerMessage::Intent {
            intent: Box::new(intent),
        }) {
        Ok(()) => {
            info!(
                "Intent queued for real-time processing: agent={}, action={}, tick={}",
                agent_id, action_type, tick_id
            );

            // 三魂元数据就地写入（与 intent 同一条消息到达，消除独立 SoulCycleReport 的丢失风险）
            if let Some(ref metadata) = soul_cycle_metadata {
                let metadata_json = serde_json::to_value(metadata).unwrap_or(serde_json::Value::Null);
                if let Err(e) = crate::db::update_soul_cycle_metadata(
                    &state.db_pool,
                    agent_id,
                    tick_id,
                    0, // pipe_seq=0：主 intent
                    &metadata_json,
                )
                .await
                {
                    warn!(
                        "三魂元数据写入失败(随intent): agent={}, tick={}, err={:#}",
                        agent_id, tick_id, e
                    );
                }

                // subsequent 占位（pipe_seq≥1）
                let world_time = metadata.world_time.clone();
                for (idx, (act_type, act_data, chaos, dream)) in subsequent_summaries.iter().enumerate() {
                    let pipe_seq = (idx + 1) as i32;
                    let placeholder = cyber_jianghu_protocol::SoulCycleMetadata {
                        world_time: world_time.clone(),
                        cycles: vec![cyber_jianghu_protocol::SoulCycleAttempt {
                            attempt: 0,
                            renhun: cyber_jianghu_protocol::RenhunReport {
                                narrative: Some("后续意图".to_string()),
                                thought_log: None,
                                earth_tool_calls: None,
                            },
                            tianhun: cyber_jianghu_protocol::TianhunReport {
                                result: Some("approved".to_string()),
                                layers: vec![
                                    cyber_jianghu_protocol::LayerReport {
                                        layer: "layer1".to_string(),
                                        passed: true,
                                        detail: None,
                                    },
                                    cyber_jianghu_protocol::LayerReport {
                                        layer: "layer2".to_string(),
                                        passed: true,
                                        detail: None,
                                    },
                                    cyber_jianghu_protocol::LayerReport {
                                        layer: "layer3".to_string(),
                                        passed: true,
                                        detail: None,
                                    },
                                ],
                                reason: None,
                            },
                            final_intent: Some(cyber_jianghu_protocol::FinalIntentReport {
                                intent_id: None,
                                action_type: Some(act_type.clone()),
                                action_data: act_data.clone(),
                                pipeline_actions: None,
                                chaos_marker: chaos.clone(),
                                dream_marker: dream.clone(),
                            }),
                            model_id: None,
                        }],
                        immediate_intents: vec![],
                    };
                    let ph_json = serde_json::to_value(&placeholder).unwrap_or(serde_json::Value::Null);
                    if let Err(e) = crate::db::update_soul_cycle_metadata(
                        &state.db_pool,
                        agent_id,
                        tick_id,
                        pipe_seq,
                        &ph_json,
                    )
                    .await
                    {
                        warn!(
                            "后续意图占位写入失败: agent={}, tick={}, pipe_seq={}, err={:#}",
                            agent_id, tick_id, pipe_seq, e
                        );
                    }
                }
            }
        }
        Err(e) => {
            warn!(
                "Intent queue full or closed: agent={}, error={}",
                agent_id, e
            );
            return reject_and_notify("Intent queue full, server busy".into()).await;
        }
    }

    Ok(())
}

/// 处理对话消息
///
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
                        &state.agent_to_device_map,
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
                        &state.agent_to_device_map,
                    )
                    .await?;
                    super::broadcast::forward_dialogue_message(
                        agent_b,
                        started_msg,
                        &state.connection_manager,
                        &state.agent_to_device_map,
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
                        &state.agent_to_device_map,
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
                        &state.agent_to_device_map,
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
                        &state.agent_to_device_map,
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
            // 发送错误消息给发起者（通过 agent_to_device_map 解析 device_id）
            let error_msg = ServerMessage::Error {
                code: cyber_jianghu_protocol::ERROR_CODE_DIALOGUE_FAILED.to_string(),
                message: format!("Dialogue failed: {}", e),
                current_tick_id: None,
            };
            let json = serde_json::to_string(&error_msg)?;
            let device_id = {
                let agent_to_device = state.agent_to_device_map.read().await;
                agent_to_device.get(&agent_id).copied()
            };
            if let Some(device_id) = device_id {
                let connections = state.connection_manager.read().await;
                if let Some(connection) = connections.get(&device_id)
                    && let Err(e) = connection.send(Message::Text(json.into())).await
                {
                    tracing::warn!(
                        "ws connection.send（broadcast）失败（receiver 可能已 drop）：{e:?}"
                    );
                }
            }
        }
    }

    Ok(())
}

/// 处理三魂循环元数据上报
///
/// Agent 在 intent 发送后通过 WebSocket SoulCycleReport 消息上报三魂循环详情。
/// Server 将元数据关联到同一 tick 的 agent_action_logs 记录。
async fn handle_soul_cycle_report(
    device_id: uuid::Uuid,
    msg_agent_id: Option<uuid::Uuid>,
    tick_id: i64,
    pipe_seq: i32,
    metadata: &SoulCycleMetadata,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 确定最终的 agent_id（与 handle_intent 相同逻辑：比较 device_id）
    let agent_id = match msg_agent_id {
        Some(id) if id != uuid::Uuid::nil() => {
            let owner_device_id: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT device_id FROM agents WHERE agent_id = $1")
                    .bind(id)
                    .fetch_optional(&state.db_pool)
                    .await
                    .context("查询 Agent 归属失败")?;

            match owner_device_id {
                Some(owner) if owner == device_id => id,
                Some(_) => {
                    warn!(
                        "SoulCycleReport: Agent ownership mismatch: agent={}, device={}",
                        id, device_id
                    );
                    return Err("无权操作此角色".into());
                }
                None => return Err("Agent 不存在".into()),
            }
        }
        _ => {
            // nil / None → 通过 device_id 查找当前 agent
            match crate::db::get_agent_by_device_id(&state.db_pool, device_id).await {
                Ok(Some(agent)) => agent.agent_id,
                Ok(None) => return Err("无关联角色".into()),
                Err(e) => return Err(format!("查询角色失败: {}", e).into()),
            }
        }
    };

    debug!(
        "收到三魂循环元数据：agent={}, tick={}, attempts={}",
        agent_id,
        tick_id,
        metadata.cycles.len()
    );

    // 将 metadata 序列化为 JSON
    let metadata_json = serde_json::to_value(metadata).context("序列化三魂循环元数据失败")?;

    // 更新 agent_action_logs 表
    if let Err(e) = crate::db::update_soul_cycle_metadata(
        &state.db_pool,
        agent_id,
        tick_id,
        pipe_seq,
        &metadata_json,
    )
    .await
    {
        warn!(
            "写入三魂循环元数据失败: agent={}, tick={}, err={:#}",
            agent_id, tick_id, e
        );
    }

    Ok(())
}

/// 处理每日 LLM 日志摘要上报
///
/// Agent 通过 WebSocket DailySummary 消息上报游戏日结束时的 LLM 事件摘要。
/// Server 注入 created_at 时间戳（服务器权威时间），然后 UPSERT 到数据库。
async fn handle_daily_summary(
    device_id: uuid::Uuid,
    game_day: i64,
    summary: &str,
    state: &Arc<crate::state::AppState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 通过 device_id 查找当前 agent
    let agent_id = match crate::db::get_agent_by_device_id(&state.db_pool, device_id).await {
        Ok(Some(agent)) => agent.agent_id,
        Ok(None) => return Err("无关联角色".into()),
        Err(e) => return Err(format!("查询角色失败: {}", e).into()),
    };

    // Server 注入时间戳（服务器权威时间，非客户端）
    let created_at = chrono::Utc::now().timestamp_millis();

    debug!(
        "收到每日摘要：agent_id={}, game_day={}, summary_len={}",
        agent_id,
        game_day,
        summary.len()
    );

    if let Err(e) = crate::db::upsert_agent_daily_summary(
        &state.db_pool,
        agent_id,
        game_day,
        summary,
        created_at,
    )
    .await
    {
        error!(
            "写入每日摘要失败: agent_id={}, game_day={}, err={}",
            agent_id, game_day, e
        );
        return Err(format!("写入每日摘要失败: {}", e).into());
    }

    info!(
        "每日摘要已存储: agent_id={}, game_day={}",
        agent_id, game_day
    );

    Ok(())
}

/// 加载并规范化 Agent 初始背包物品。
///
/// 之前在 DB 失败时静默回退为 `vec![]`，会让 Agent 在"空背包"假状态下决策。
/// 现改为：任何错误（DB 抖动、列漂移、Schema 异常）必须显式返回 Err，
/// 由 caller 决定是否关闭连接。
pub(crate) async fn load_initial_inventory(
    db_pool: &sqlx::PgPool,
    agent_id: uuid::Uuid,
) -> anyhow::Result<Vec<crate::models::InventoryItem>> {
    let raw_items = InventoryManager::get_all_items(db_pool, agent_id)
        .await
        .context("query agent inventory")?;
    Ok(raw_items
        .into_iter()
        .map(|item| {
            let config = ItemRegistry::get(&item.item_id);
            let name = config
                .as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| item.item_id.clone());
            let item_type = config
                .as_ref()
                .map(|c| c.item_type.clone())
                .unwrap_or_default();
            crate::models::InventoryItem {
                item_id: item.item_id,
                name,
                quantity: item.quantity,
                is_equipped: item.is_equipped,
                item_type,
            }
        })
        .collect())
}

/// 加载并规范化当前节点地面物品。
pub(crate) async fn load_nearby_ground_items(
    db_pool: &sqlx::PgPool,
    node_id: &str,
) -> anyhow::Result<Vec<cyber_jianghu_protocol::SceneItem>> {
    let raw_items = crate::db::get_ground_items_by_node(db_pool, node_id)
        .await
        .context("query ground items")?;
    Ok(raw_items
        .into_iter()
        .map(|gi| {
            let config = ItemRegistry::get(&gi.item_id);
            let name = config
                .as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| gi.item_id.clone());
            let item_type = config
                .as_ref()
                .map(|c| c.item_type.clone())
                .unwrap_or_default();
            cyber_jianghu_protocol::SceneItem {
                item_id: gi.item_id,
                name,
                quantity: gi.quantity,
                item_type,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::rebind_device_agent;
    use std::collections::HashMap;
    use uuid::Uuid;

    #[test]
    fn test_rebind_device_agent_removes_stale_agents_for_same_device() {
        let device_id = Uuid::new_v4();
        let stale_agent = Uuid::new_v4();
        let current_agent = Uuid::new_v4();
        let other_device = Uuid::new_v4();
        let unrelated_agent = Uuid::new_v4();
        let mut map = HashMap::from([(stale_agent, device_id), (unrelated_agent, other_device)]);

        rebind_device_agent(&mut map, current_agent, device_id);

        assert_eq!(map.get(&current_agent), Some(&device_id));
        assert!(!map.contains_key(&stale_agent));
        assert_eq!(map.get(&unrelated_agent), Some(&other_device));
    }

    /// 验证 P1-7：初始背包加载失败时，helper 显式返回 Err，
    /// 不再向 caller 静默返回空 Vec。
    /// 真实 DB 抖动由 caller 决定是否关闭 WebSocket。
    #[tokio::test]
    async fn test_load_initial_inventory_propagates_db_error() {
        // 构造一个连接到无效地址的 PgPool —— 任何 query 都会失败
        let pool: sqlx::PgPool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(50))
            .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/nonexistent")
            .expect("lazy pool init should not connect");

        let result = super::load_initial_inventory(&pool, Uuid::new_v4()).await;
        assert!(
            result.is_err(),
            "load_initial_inventory must not silently fall back to empty Vec on DB error"
        );
    }

    /// 验证 P1-7：地面物品加载失败也必须显式 Err。
    #[tokio::test]
    async fn test_load_nearby_ground_items_propagates_db_error() {
        let pool: sqlx::PgPool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(50))
            .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/nonexistent")
            .expect("lazy pool init should not connect");

        let result = super::load_nearby_ground_items(&pool, "some-node").await;
        assert!(
            result.is_err(),
            "load_nearby_ground_items must not silently fall back to empty Vec on DB error"
        );
    }
}
