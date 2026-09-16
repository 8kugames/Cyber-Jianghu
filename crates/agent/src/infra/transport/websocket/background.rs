// ============================================================================
// 后台 WebSocket 任务（独占连接的 select! 循环）
// ============================================================================

use super::*;

// ============================================================================

/// 后台 WebSocket 任务
///
/// 独占 WebSocket，使用 tokio::select! 同时处理：
/// - 接收消息（持续轮询 ws.next()，自动响应 Ping/Pong）
/// - 发送 intent（通过 mpsc channel）
///
/// WorldState 通过 watch channel 传递给主循环，
/// 其他消息通过回调处理（与原 receive_and_handle_message 逻辑一致）。
pub(super) async fn websocket_background_task(
    mut ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    state: Arc<RwLock<ConnectionState>>,
    mut intent_rx: tokio::sync::mpsc::Receiver<ClientMessage>,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    /// 读超时：server 每 30s 发 Ping，120s 无任何消息 = 连接已死
    const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

    info!("WebSocket background task started");
    let mut last_message_time = std::time::Instant::now();

    loop {
        let remaining = READ_TIMEOUT.saturating_sub(last_message_time.elapsed());

        tokio::select! {
            // 读超时：连接静默死亡（server 重启、网络断开、TCP 半开）
            _ = tokio::time::sleep(remaining) => {
                warn!(
                    "Background: 读超时 ({:?} 无消息)，连接已死",
                    last_message_time.elapsed()
                );
                if let Some(ref tx) = {
                    let guard = state.read().await;
                    guard.worldstate_tx.clone()
                }
                    && let Err(e) = tx.send(None) {
                        handle_worldstate_send_failure(tx, "read-timeout", e);
                    }
                break;
            }

            // 检查关闭信号
            res = shutdown_rx.recv() => {
                match res {
                    Ok(_) => {
                        info!("WebSocket background: shutdown signal received");
                        // 优雅关闭 WebSocket
                        let _ = ws.close(None).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Lagged shutdown signal, ignore
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }

            // 接收消息（持续轮询，自动响应 Ping）
            msg_result = ws.next() => {
                last_message_time = std::time::Instant::now();
                match msg_result {
                    Some(Ok(Message::Text(text))) => {
                        // 克隆回调（避免在处理中持有锁）
                        let (game_rules_cb, dialogue_cb, wb_rules_cb, action_update_cb, skill_update_cb, prompt_template_cb, persona_event_rules_cb, narrative_config_cb, server_msg_cb, ws_tx, reg_tx, exec_result_tx, events_tx) = {
                            let state_guard = state.read().await;
                            (
                                state_guard.game_rules_callback.clone(),
                                state_guard.dialogue_callback.clone(),
                                state_guard.world_building_rules_callback.clone(),
                                state_guard.action_update_callback.clone(),
                                state_guard.skill_update_callback.clone(),
                                state_guard.prompt_template_callback.clone(),
                                state_guard.persona_event_rules_callback.clone(),
                                state_guard.narrative_config_callback.clone(),
                                state_guard.server_msg_callback.clone(),
                                state_guard.worldstate_tx.clone(),
                                state_guard.registered_tx.clone(),
                                state_guard.execution_result_tx.clone(),
                                state_guard.events_tx.clone(),
                            )
                        };

                        match serde_json::from_str::<ServerMessage>(&text) {
                            Ok(ServerMessage::WorldState { data }) => {
                                debug!("Background: WorldState tick={}", data.tick_id);
                                // 事件流先行入队：watch 只保最新快照，若本快照被后续
                                // 广播覆盖，其 events_log（含不可重复的死亡等事件）
                                // 由主循环 drain 合并找回，不再随快照丢失
                                if let Some(ref etx) = events_tx
                                    && let Err(e) = etx.try_send(data.events_log.clone())
                                {
                                    if etx.is_closed() {
                                        debug!(
                                            "events_tx.try_send 失败：主循环未订阅，保留 sender: {e:?}"
                                        );
                                    } else {
                                        warn!(
                                            "events_tx.try_send 失败（队列满，事件批次被丢弃）: {e:?}"
                                        );
                                    }
                                }
                                if let Some(ref tx) = ws_tx
                                    && let Err(e) = tx.send(Some(data)) {
                                        handle_worldstate_send_failure(tx, "world-state-msg", e);
                                    }
                            }
                            Ok(msg @ ServerMessage::ConfigUpdate { .. }) => {
                                if let ServerMessage::ConfigUpdate {
                                    ref config_type,
                                    ref update_type,
                                    ref version,
                                    ref content,
                                    ref content_hash,
                                    ref updated_items,
                                    ref removed_items,
                                } = msg
                                {
                                    info!(
                                        "Background: ConfigUpdate type={}, config_type={:?}, v={}, +{}, -{}",
                                        update_type, config_type, version,
                                        updated_items.len(), removed_items.len()
                                    );

                                    match config_type {
                                        ConfigType::Skills => {
                                            // 目前仅支持 full update_type，增量更新暂未实现
                                            if update_type != "full" {
                                                warn!(
                                                    "ConfigUpdate: skills update_type={} not fully supported, treating as full",
                                                    update_type
                                                );
                                            }

                                            if let Ok(skills) = serde_json::from_value::<Vec<SkillContent>>(content.clone()) {
                                                if let Some(ref cb) = skill_update_cb {
                                                    cb(skills, removed_items.clone());
                                                }
                                            } else {
                                                warn!("Failed to parse skills content from ConfigUpdate");
                                            }
                                        }
                                        // 处理 actions 配置更新
                                        // 当前仅支持 full update，增量更新暂未实现
                                        // actions 通过 action_update_callback 透传整个 ServerMessage
                                        ConfigType::Actions => {
                                            if let Some(ref cb) = action_update_cb {
                                                cb(msg.clone());
                                            }
                                        }
                                        // 处理 game_rules 配置更新
                                        ConfigType::GameRules => {
                                            if let Ok(game_rules) = serde_json::from_value::<GameRules>(content.clone()) {
                                                // 更新本地缓存
                                                {
                                                    let mut guard = state.write().await;
                                                    guard.game_rules = Some(game_rules.clone());
                                                }
                                                // 调用回调
                                                if let Some(ref cb) = game_rules_cb {
                                                    cb(game_rules);
                                                }
                                            } else {
                                                warn!("Failed to parse game_rules content from ConfigUpdate");
                                            }
                                        }
                                        // 处理 world_building_rules 配置更新
                                        ConfigType::WorldBuildingRules => {
                                            if let Ok(wb_rules) = serde_json::from_value::<WorldBuildingRules>(content.clone()) {
                                                // 更新本地缓存
                                                {
                                                    let mut guard = state.write().await;
                                                    guard.world_building_rules = Some(wb_rules.clone());
                                                }
                                                // 调用回调
                                                if let Some(ref cb) = wb_rules_cb {
                                                    cb(wb_rules);
                                                }
                                            } else {
                                                warn!("Failed to parse world_building_rules content from ConfigUpdate");
                                            }
                                        }
                                        // 处理 prompt_templates 配置更新（JSON 格式 + hash skip）
                                        ConfigType::PromptTemplates => {
                                            // hash skip: 内容未变则跳过更新
                                            let should_update = {
                                                let state_guard = state.read().await;
                                                match (content_hash.as_ref(), state_guard.prompt_template_hash.as_ref()) {
                                                    (Some(new_hash), Some(old_hash)) => new_hash != old_hash,
                                                    _ => true,
                                                }
                                            };
                                            if should_update {
                                                // 无论解析成功与否，先记录 hash，防止相同坏数据反复重试
                                                if let Some(hash) = content_hash.as_ref() {
                                                    let mut state_guard = state.write().await;
                                                    state_guard.prompt_template_hash = Some(hash.clone());
                                                }
                                                if let Ok(config) = cyber_jianghu_protocol::PromptTemplateConfig::from_json_value(content.clone()) {
                                                    // 标记 WS 已成功投递，HTTP 拉取可跳过
                                                    {
                                                        let mut state_guard = state.write().await;
                                                        state_guard.prompt_template_received = true;
                                                    }
                                                    if let Some(ref cb) = prompt_template_cb {
                                                        cb(config);
                                                    }
                                                } else {
                                                    warn!("Failed to parse prompt_templates JSON from ConfigUpdate");
                                                }
                                            } else {
                                                debug!("prompt_templates skip: hash unchanged");
                                            }
                                        }
                                        // 处理 persona_event_rules 配置更新
                                        ConfigType::PersonaEventRules => {
                                            #[derive(serde::Deserialize)]
                                            struct RulesJson {
                                                rules: Vec<crate::component::persona::TraitMappingRule>,
                                            }
                                            match serde_json::from_value::<RulesJson>(content.clone()) {
                                                Ok(parsed) => {
                                                    if let Some(ref cb) = persona_event_rules_cb {
                                                        cb(parsed.rules);
                                                    }
                                                }
                                                Err(e) => {
                                                    warn!(
                                                        "Failed to parse persona_event_rules content from ConfigUpdate: {}",
                                                        e
                                                    );
                                                }
                                            }
                                        }
                                        // 处理 narrative_config 配置更新
                                        ConfigType::NarrativeConfig => {
                                            if let Ok(nc) = serde_json::from_value::<cyber_jianghu_protocol::NarrativeConfig>(content.clone()) {
                                                if let Some(ref cb) = narrative_config_cb {
                                                    cb(nc, content_hash.clone());
                                                }
                                            } else {
                                                warn!("Failed to parse narrative_config content from ConfigUpdate");
                                            }
                                        }
                                    }
                                }
                                if let Some(ref cb) = server_msg_cb {
                                    cb(msg);
                                }
                            }
                            Ok(msg @ ServerMessage::Dialogue { .. }) => {
                                debug!("Background: Dialogue received");
                                if let ServerMessage::Dialogue { ref message } = msg
                                    && let Some(ref cb) = dialogue_cb
                                {
                                    cb(message.clone());
                                }
                                if let Some(ref cb) = server_msg_cb {
                                    cb(msg);
                                }
                            }
                            Ok(msg @ ServerMessage::ImmediateEvent { .. }) => {
                                debug!("Background: ImmediateEvent received");
                                if let Some(ref cb) = server_msg_cb {
                                    cb(msg);
                                }
                            }
                            Ok(ServerMessage::ExecutionResult {
                                tick_id,
                                intent_id,
                                success,
                                error,
                                state_change_summary,
                                governance_code,
                            }) => {
                                debug!(
                                    "Background: ExecutionResult tick={}, intent={}, success={}",
                                    tick_id, intent_id, success
                                );
                                if let Some(ref tx) = exec_result_tx {
                                    let _ = tx.try_send(ExecutionResultData {
                                        tick_id,
                                        intent_id,
                                        success,
                                        error: error.clone(),
                                        state_change_summary: state_change_summary.clone(),
                                        governance_code,
                                    });
                                }
                            }
                            Ok(ServerMessage::Error {
                                code,
                                message,
                                current_tick_id,
                            }) => {
                                let is_tick_mismatch =
                                    code == cyber_jianghu_protocol::ERROR_CODE_TICK_MISMATCH;

                                if let Some(ref cb) = server_msg_cb {
                                    cb(ServerMessage::Error {
                                        code: code.clone(),
                                        message: message.clone(),
                                        current_tick_id,
                                    });
                                }

                                if is_tick_mismatch {
                                    error!("Background: Tick mismatch: {}", message);
                                    // tick mismatch 自恢复：下一个 tick 的 WorldState 会自然到来
                                } else {
                                    warn!("Background: Server error: {}", message);
                                }
                            }
                            Ok(ServerMessage::Registered {
                                agent_id,
                                game_rules,
                                world_building_rules,
                                is_alive,
                                agent_name,
                                narrative_config,
                                narrative_config_hash,
                            }) => {
                                info!("Background: Registered agent_id={}, alive={}", agent_id, is_alive);
                                // 保存注册数据到 watch channel
                                if let Some(ref tx) = reg_tx
                                    && let Err(e) = tx.send(Some(RegistrationData {
                                        agent_id,
                                        game_rules,
                                        world_building_rules,
                                        agent_name,
                                        is_alive,
                                        narrative_config,
                                        narrative_config_hash,
                                    })) {
                                        tracing::warn!("reg_tx.send 失败（receiver 可能已 drop）：{e:?}");
                                    }
                            }
                            Ok(msg @ ServerMessage::AgentDied { .. }) => {
                                if let ServerMessage::AgentDied {
                                    agent_id,
                                    cause,
                                    description,
                                    ..
                                } = &msg
                                {
                                    let current_agent_id = {
                                        let guard = state.read().await;
                                        guard.agent_id
                                    };
                                    if current_agent_id == Some(*agent_id) {
                                        warn!("Agent {} died: {} - {}", agent_id, cause, description);
                                        if let Some(ref cb) = server_msg_cb {
                                            cb(msg.clone());
                                        }
                                    }
                                }
                            }
                            Ok(ServerMessage::Pong { .. }) => {
                                debug!("Background: Pong received");
                            }
                            Err(e) => {
                                warn!("Background: Parse error: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Ping(_))) => {
                        // tungstenite 自动回复 Pong
                    }
                    Some(Ok(Message::Pong(_))) => {
                        debug!("Background: Pong received");
                    }
                    Some(Ok(Message::Close(_))) => {
                        warn!("Background: Server closed connection");
                        if let Some(ref tx) = {
                            let guard = state.read().await;
                            guard.worldstate_tx.clone()
                        }
                            && let Err(e) = tx.send(None) {
                                tracing::warn!("worldstate_tx.send(None) [closed conn] 失败（receiver 可能已 drop）：{e:?}");
                            }
                        break;
                    }
                    Some(Err(e)) => {
                        error!("Background: WebSocket error: {}", e);
                        if let Some(ref tx) = {
                            let guard = state.read().await;
                            guard.worldstate_tx.clone()
                        }
                            && let Err(e) = tx.send(None) {
                                tracing::warn!("worldstate_tx.send(None) [ws error] 失败（receiver 可能已 drop）：{e:?}");
                            }
                        break;
                    }
                    None => {
                        warn!("Background: Stream ended");
                        if let Some(ref tx) = {
                            let guard = state.read().await;
                            guard.worldstate_tx.clone()
                        }
                            && let Err(e) = tx.send(None) {
                                tracing::warn!("worldstate_tx.send(None) [stream ended] 失败（receiver 可能已 drop）：{e:?}");
                            }
                        break;
                    }
                    _ => {}
                }
            }
            // 发送 ClientMessage（Intent、SoulCycleReport 等统一通道）
            Some(msg) = intent_rx.recv() => {
                let json = match serde_json::to_string(&msg) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Background: Failed to serialize message: {}", e);
                        continue;
                    }
                };

                if let Err(e) = ws.send(Message::Text(json.into())).await {
                    error!("Background: Failed to send message: {}", e);
                    break;
                }
                debug!("Background: Sent message via unified channel");
            }
        }
    }

    info!("WebSocket background task exiting");
}
