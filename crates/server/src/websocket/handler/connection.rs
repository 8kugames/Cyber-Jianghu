// ============================================================================
// WebSocket 连接生命周期（handle_websocket）
// ============================================================================
//
// 连接建立 → 注册握手（game_rules/技能/prompt/narrative 下发）→ 消息循环 → 清理。

use super::*;

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
pub(super) async fn handle_websocket(
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

        let survival = super::super::types::SurvivalConfig {
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
        let narrative_config = state.game_data.get().narrative.clone();
        let narrative_config_hash = cyber_jianghu_protocol::payload_hash(&narrative_config);

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
            narrative_config: Some(narrative_config),
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
            let config_update = ServerMessage::config_update_full_value(
                cyber_jianghu_protocol::ConfigType::Skills,
                "1.0.0",
                serde_json::to_value(&skill_contents).unwrap_or_default(),
                None,
            );

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
            let config_update = ServerMessage::config_update_full_value(
                cyber_jianghu_protocol::ConfigType::PromptTemplates,
                pt_cache.version.clone(),
                pt_cache.json_value.clone(),
                Some(pt_cache.hash.clone()),
            );

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
                    let config_update = ServerMessage::config_update_full_value(
                        cyber_jianghu_protocol::ConfigType::PersonaEventRules,
                        "1.0",
                        json_value,
                        None,
                    );

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
        let nc = state.game_data.get().narrative.clone();
        let config_update = ServerMessage::config_update_full(
            cyber_jianghu_protocol::ConfigType::NarrativeConfig,
            "1.0",
            &nc,
        );

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
