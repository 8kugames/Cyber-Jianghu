// ============================================================================
// 连接生命周期管理
// ============================================================================
//
// 处理 Agent 的连接、主循环和关闭
// 重连逻辑在 reconnect.rs 中
// ============================================================================

use anyhow::Result;
use cyber_jianghu_protocol::{ServerMessage, WorldTime};
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::reconnect::{save_character_config_to_fs, should_log_retry};
use crate::config::CharacterStatus;
use crate::infra::transport::ConnectError;
use crate::models::Intent;

impl super::Agent {
    /// 运行 Agent 主循环
    ///
    /// 持续接收世界状态，做出决策，发送意图
    pub async fn run(&mut self) -> Result<()> {
        // 检查角色状态：若已死亡或已归隐，跳过服务器连接
        let skip_connection = self.death_reported
            || self
                .character_config
                .as_ref()
                .map(|c| c.status != CharacterStatus::Alive)
                .unwrap_or(false);

        if skip_connection {
            if let Some(ref character) = self.character_config {
                warn!(
                    "Agent '{}' status is {:?}, waiting for rebirth",
                    character.name, character.status
                );
            } else {
                warn!("No active character, waiting for character creation");
            }
            // 保持进程存活，等待 reconnect_rx 触发重连
            self.wait_for_rebirth().await?;
            return Ok(());
        }

        // 初始连接：无限重试（带日志采样）
        let mut connect_attempt = 0u32;
        loop {
            connect_attempt += 1;
            match self.client.connect().await {
                Ok(()) => break,
                Err(ConnectError::AuthFailed) => {
                    warn!(
                        "WebSocket auth failed (attempt {}), refreshing token...",
                        connect_attempt
                    );
                    match self.refresh_device_token().await {
                        Ok(()) => {
                            info!("Token refreshed, retrying connection...");
                            continue;
                        }
                        Err(e) => {
                            if should_log_retry(connect_attempt) {
                                warn!(
                                    "Token refresh failed (attempt {}): {}, 5秒后重试...",
                                    connect_attempt, e
                                );
                            }
                        }
                    }
                }
                Err(ConnectError::ConnectionFailed(e)) => {
                    if should_log_retry(connect_attempt) {
                        warn!(
                            "连接游戏服务器失败 (尝试 {}): {}, 5秒后重试...",
                            connect_attempt, e
                        );
                    }
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
        info!("Agent '{}' connected to server", self.character_name());

        // 设置游戏规则更新回调
        let agent_name_for_callback = self.character_name().to_string();
        self.client
            .set_game_rules_callback(Arc::new(move |game_rules| {
                info!(
                    "Agent '{}' received game rules update: version {}",
                    agent_name_for_callback, game_rules.version
                );
                // 注意：配置持久化由外部配置管理系统处理
            }))
            .await;

        // 设置对话消息回调（如果启用了对话系统）
        if self.dialogue_client.is_some() {
            let dialogue_client = self.dialogue_client.clone();
            let agent_name_for_dialogue = self.character_name().to_string();
            self.client
                .set_dialogue_callback(Arc::new(move |message| {
                    debug!(
                        "Agent '{}' received dialogue message",
                        agent_name_for_dialogue
                    );
                    if let Some(ref dc) = dialogue_client {
                        dc.handle_message(message);
                    }
                }))
                .await;
            info!(
                "Dialogue callback set for agent '{}'",
                self.character_name()
            );
        }

        // 设置世界观规则更新回调（如果启用了验证器）
        if self.validator.is_some() {
            let validator = self.validator.clone();
            let agent_name_for_rules = self.character_name().to_string();
            self.client
                .set_world_building_rules_callback(Arc::new(move |rules| {
                    info!(
                        "Agent '{}' received world building rules update: version {}",
                        agent_name_for_rules, rules.version
                    );
                    if let Some(ref v) = validator {
                        // 使用 tokio::spawn 因为回调不是 async
                        let v_clone = v.clone();
                        let rules_clone = rules.clone();
                        tokio::spawn(async move {
                            v_clone.update_rules(rules_clone).await;
                        });
                    }
                }))
                .await;
            info!(
                "World building rules callback set for agent '{}'",
                self.character_name()
            );
        }

        // 等待注册确认（包含游戏规则）
        // Ok(None) = agent_id 为 nil，等待角色注册（保持连接，不 close/reconnect）
        let (agent_id, game_rules, registered_name, is_alive) =
            match self.client.wait_for_registration().await {
                Ok(Some((id, rules, name, alive))) => (id, rules, name, alive),
                Ok(None) => {
                    info!(
                        "Agent '{}' 等待角色注册（保持连接）...",
                        self.character_name()
                    );
                    self.death_reported = true;
                    if let Some(ref api_state) = self.http_api_state {
                        api_state
                            .is_dead
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    self.wait_for_rebirth().await?;
                    return Ok(());
                }
                Err(e) => return Err(e),
            };
        // 重置重试计数器
        self.reconnect_backoff = 0;
        info!("Agent '{}' registered with server", self.character_name());
        info!("Server-assigned Agent ID: {}", agent_id);

        // 使用服务器返回的角色名更新 Agent 名称追踪
        if let Some(ref name) = registered_name {
            self.server_assigned_name = Some(name.clone());
            self.reload_character_persona(agent_id, name);
            info!("已更新 agent 名称为: {}", name);
        }

        // 自动重建本地 character.yaml（解决 agent-server 状态不同步问题）
        // 场景：服务器已有角色但本地文件丢失（如清除缓存、目录迁移）
        if self.character_config.is_none() && !agent_id.is_nil() {
            let server_dir = self.config.server_dir(&self.config.server.ws_url);
            let characters_dir = server_dir.join("characters");
            let char_dir = characters_dir.join(agent_id.to_string());
            let char_yaml = char_dir.join("character.yaml");

            if !char_yaml.exists() {
                let name = registered_name.as_deref().unwrap_or("未知");
                let reconstructed = crate::config::CharacterConfig {
                    agent_id: Some(agent_id),
                    name: name.to_string(),
                    status: crate::config::CharacterStatus::Alive,
                    server_url: Some(self.config.server.http_url.clone()),
                    registered_at: Some(chrono::Utc::now()),
                    ..Default::default()
                };

                if let Err(e) = (|| -> anyhow::Result<()> {
                    std::fs::create_dir_all(&char_dir)?;
                    reconstructed.save_to_file(&char_yaml)?;
                    Ok(())
                })() {
                    warn!("自动重建 character.yaml 失败: {}", e);
                } else {
                    info!("已自动重建本地角色配置: {} ({})", name, agent_id);
                    self.character_config = Some(reconstructed);
                }
            }
        }

        // agent_id 为零 = 角色已归隐，跳过主循环，直接触发死亡/转生流程
        if agent_id == Uuid::nil() {
            warn!(
                "Agent '{}' retired (agent_id is nil)",
                self.character_name()
            );
            self.death_reported = true;

            if let Some(ref api_state) = self.http_api_state {
                api_state
                    .is_dead
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                let death_msg = ServerMessage::AgentDied {
                    agent_id: Uuid::nil(),
                    cause: "retired".to_string(),
                    description: "角色已归隐，请创建新角色".to_string(),
                    location: String::new(),
                    tick_id: 0,
                    died_at: chrono::Utc::now().timestamp_millis(),
                    rebirth_delay_ticks: 0,
                };
                let _ = api_state.death_event_tx.send(death_msg);
            }

            // 归隐后保持进程存活，等待创建新角色
            self.wait_for_rebirth().await?;
            return Ok(());
        }

        // 服务器返回 agent_id 但 is_alive=false：断连期间角色死亡
        // 此时 agent_id 有效但角色已不在，需要 rebirth
        if !is_alive {
            warn!(
                "Agent '{}' ({}) died during disconnect (is_alive=false)",
                self.character_name(),
                agent_id
            );
            self.death_reported = true;
            if let Some(ref api_state) = self.http_api_state {
                api_state
                    .is_dead
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                let death_msg = ServerMessage::AgentDied {
                    agent_id,
                    cause: "disconnect_death".to_string(),
                    description: "角色在断连期间死亡，请通过 rebirth 创建新角色".to_string(),
                    location: String::new(),
                    tick_id: 0,
                    died_at: chrono::Utc::now().timestamp_millis(),
                    rebirth_delay_ticks: 0,
                };
                let _ = api_state.death_event_tx.send(death_msg);
            }

            // 持久化死亡状态
            if let Some(ref mut char_cfg) = self.character_config {
                char_cfg.status = crate::config::CharacterStatus::Dead;
                if let Some(ref api_state) = self.http_api_state {
                    let characters_dir = api_state.character_dir.read().await.clone();
                    if let Err(e) = save_character_config_to_fs(char_cfg, &characters_dir) {
                        warn!("Failed to persist disconnect-death status: {}", e);
                    }
                }
            }

            self.wait_for_rebirth().await?;
            return Ok(());
        }

        if let Some(ref callback) = self.registration_callback {
            callback(agent_id);
        }
        info!(
            "Received game rules: version {}, {} actions, {} initial items",
            game_rules.version,
            game_rules.available_actions.len(),
            game_rules.initial_items.len()
        );

        // 更新游戏规则
        self.config.update_game_rules(game_rules.clone());

        // 绑定即时意图通道到 WebSocket 的 immediate_msg_tx
        if let Some(ref handler) = self.immediate_handler {
            if let Some(tx) = self.client.immediate_msg_sender().await {
                handler.replace_intent_channel(tx).await;
            } else {
                warn!("WebSocket immediate_msg_tx 不可用，即时回应将使用临时 channel");
            }
        }

        // 设置 Server 消息回调（链式：lifecycle 处理 + binary 回调透传）
        // 保留 binary 设置的回调（Cognitive: AgentDied 处理; Claw: OpenClaw 消息转发）
        let prev_callback = self.client.get_server_msg_callback().await;
        let immediate_handler = self.immediate_handler.clone();
        let error_feedback = self.server_error_feedback.clone();
        let event_buffer = self.immediate_event_buffer.clone();
        let callback: Arc<dyn Fn(ServerMessage) + Send + Sync> =
            Arc::new(move |msg: ServerMessage| {
                // 1. 验证错误反馈
                if let ServerMessage::Error { code, message, .. } = &msg
                    && code == cyber_jianghu_protocol::ERROR_CODE_ACTION_FAILED
                {
                    let reason = message.clone();
                    let feedback = error_feedback.clone();
                    tokio::spawn(async move {
                        let mut guard = feedback.lock().await;
                        *guard = Some(reason);
                    });
                }
                // 2. ImmediateEvent: 即时决策 + 写入工作记忆
                if let ServerMessage::ImmediateEvent { event, .. } = &msg {
                    // 2a. 写入即时事件缓冲区（主循环消费后写入工作记忆）
                    let evt = event.clone();
                    let buf = event_buffer.clone();
                    tokio::spawn(async move {
                        let mut guard = buf.lock().await;
                        guard.push(evt);
                    });
                    // 2b. 转给即时事件处理器（RespondNow/Defer/Ignore）
                    if let Some(ref handler) = immediate_handler {
                        let h = handler.clone();
                        let msg = msg.clone();
                        tokio::spawn(async move {
                            h.handle_server_message(msg).await;
                        });
                    }
                }
                // 2c. Dialogue（whisper 密语）：写入工作记忆
                if let ServerMessage::Dialogue { message, .. } = &msg {
                    use cyber_jianghu_protocol::DialogueMessage;
                    let desc = match message {
                        DialogueMessage::Request { opening_remark, .. } => {
                            format!("收到密语请求: {}", opening_remark)
                        }
                        DialogueMessage::Content { content, .. } => {
                            format!("密语内容: {}", content)
                        }
                        DialogueMessage::Accept { .. } => "密语对话已接受".to_string(),
                        DialogueMessage::Reject { reason, .. } => {
                            format!("密语对话被拒绝: {}", reason.as_deref().unwrap_or("无理由"))
                        }
                        DialogueMessage::End { .. } => "密语对话已结束".to_string(),
                    };
                    let buf = event_buffer.clone();
                    tokio::spawn(async move {
                        let world_event = cyber_jianghu_protocol::WorldEvent {
                            event_type: cyber_jianghu_protocol::WorldEventType::PrivateDialogue,
                            tick_id: 0,
                            description: desc,
                            metadata: serde_json::json!({}),
                        };
                        let mut guard = buf.lock().await;
                        guard.push(world_event);
                    });
                }
                // 3. 透传给 binary 回调（AgentDied 处理、Claw 模式 OpenClaw 转发等）
                if let Some(ref prev) = prev_callback {
                    prev(msg);
                }
            });
        self.client.set_server_msg_callback(callback).await;
        info!("Server 消息回调已注册（即时事件 + 验证错误 + 链式透传）");

        loop {
            tokio::select! {
                // 检查重连请求（热切换）
                Ok(req) = async {
                    if let Some(ref mut rx) = self.reconnect_rx {
                        rx.recv().await
                    } else {
                        // 非 Claw 模式，永远等待
                        std::future::pending().await
                    }
                } => {
                    info!("[main] 收到重连请求: {}", req.ws_url);
                    // 推断 HTTP URL
                    let http_url = crate::config::ws_to_http_url(&req.ws_url);
                    // 更新客户端 URL
                    self.client.update_server_url(req.ws_url.clone(), http_url).await;
                    // 触发重连
                    self.reconnect().await?;
                    continue;
                }

                // 接收世界状态
                result = self.client.receive_world_state() => {
                    let world_state = match result {
                        Ok(state) => state,
                        Err(e) => {
                            // 连接断开或 channel 错误，重连
                            // tick mismatch 不走此路径（自恢复：下一个 tick 的 WorldState 自然到来）
                            error!("Failed to receive world state: {}", e);
                            self.reconnect().await?;
                            continue;
                        }
                    };

                    // 更新即时事件处理器 tick_id
                    if let Some(ref handler) = self.immediate_handler {
                        handler.set_tick_id(world_state.tick_id).await;
                    }

                    // 更新 HTTP API 状态（供 Web Panel 查询）
                    if let Some(ref api_state) = self.http_api_state {
                        let mut current = api_state.current_state.write().await;
                        *current = Some(world_state.clone());

                        let mut last_update = api_state.last_state_update.write().await;
                        *last_update = Some(std::time::Instant::now());

                        // 异步更新关系叙事（不阻塞）
                        api_state.maybe_update_narratives(&world_state).await;
                    }

                    // 更新角色配置的最近连接时间并持久化
                    if let Some(ref mut char_cfg) = self.character_config {
                        char_cfg.last_connected_real_time = Some(chrono::Utc::now());
                        char_cfg.last_connected_world_time = Some(world_state.world_time.clone());

                        // 异步保存到磁盘（不阻塞主循环）
                        if let Some(ref api_state) = self.http_api_state {
                            let char_cfg_clone = char_cfg.clone();
                            let characters_dir = api_state.character_dir.read().await.clone();
                            tokio::spawn(async move {
                                if let Err(e) =
                                    save_character_config_to_fs(&char_cfg_clone, &characters_dir)
                                {
                                    warn!("Failed to save character last_connected time: {}", e);
                                }
                            });
                        }
                    }

                    // 1.5 检查是否死亡（只报告一次）
                    if !self.death_reported
                        && let Some(death_event) = world_state.events_log.iter().find(|e| {
                            e.event_type == cyber_jianghu_protocol::WorldEventType::DeathNotification
                        }) {
                            warn!(
                                "Agent '{}' has died: {}",
                                self.character_name(), death_event.description
                            );
                            self.death_reported = true;
                            if let Some(ref api_state) = self.http_api_state {
                                api_state.is_dead.store(true, std::sync::atomic::Ordering::Relaxed);
                            }

                            // 持久化死亡状态到 character.yaml（确保世界树显示正确）
                            if let Some(ref mut char_cfg) = self.character_config {
                                char_cfg.status = crate::config::CharacterStatus::Dead;
                                if let Some(ref api_state) = self.http_api_state {
                                    let characters_dir = api_state.character_dir.read().await.clone();
                                    if let Err(e) = save_character_config_to_fs(char_cfg, &characters_dir) {
                                        warn!("Failed to persist death status: {}", e);
                                    }
                                }
                            }

                            // 死亡后不退出，等待转生：
                            // - Cognitive 模式：继续循环，等待 rebirth handler 触发重连
                            // - Claw 模式：OpenClaw 已收到 AgentDied 信号，会通过 reconnect_rx 触发重连
                            continue;
                        }

                    // 1.5 清除上一 tick 的 rejection reason（在消费新反馈之前）
                    self.last_rejection_reason = None;

                    // 1.6 消费 Server 验证错误反馈（由 Fn callback 异步写入）
                    {
                        let mut guard = self.server_error_feedback.lock().await;
                        if let Some(reason) = guard.take() {
                            warn!("Server 验证错误反馈: {}", reason);
                            self.last_rejection_reason = Some(super::Agent::narrativize_rejection(&reason));
                        }
                    }

                    // 1.7 消费即时事件缓冲区（ImmediateEvent 即时写入工作记忆）
                    let immediate_events = {
                        let mut guard = self.immediate_event_buffer.lock().await;
                        if guard.is_empty() { Vec::new() } else { guard.drain(..).collect() }
                    };
                    if !immediate_events.is_empty() {
                        debug!("消费 {} 个即时事件到工作记忆", immediate_events.len());
                        if let Err(e) = self.process_events(&immediate_events).await {
                            warn!("即时事件写入记忆失败: {}", e);
                        }
                    }

                    // 2. 处理事件并更新记忆
                    if let Err(e) = self.process_events(&world_state.events_log).await {
                        warn!("Failed to process events into memory: {}", e);
                    }

                    // 3. 每 FORGETTING_INTERVAL_TICKS tick 运行遗忘机制
                    if world_state.tick_id % super::FORGETTING_INTERVAL_TICKS == 0
                        && let Err(e) = self.run_forgetting(world_state.tick_id).await {
                            warn!("Failed to run forgetting mechanism: {}", e);
                        }

                    // 4. 构建增强的世界状态（包含记忆上下文 + deferred 对话）
                    let mut memory_context = self.get_memory_context().await;
                    // 注入延迟处理的即时对话（DeferToMainTick 事件）
                    if let Some(ref handler) = self.immediate_handler {
                        let deferred = handler.get_deferred_events().await;
                        if !deferred.is_empty() {
                            let deferred_ctx: Vec<String> = deferred.iter()
                                .filter_map(|e| {
                                    let content = e.metadata.get("content")
                                        .and_then(|v| v.as_str()).unwrap_or("");
                                    if content.is_empty() { None }
                                    else { Some(format!("[有人对你说: {}]", content)) }
                                })
                                .collect();
                            if !deferred_ctx.is_empty() {
                                let deferred_section = format!(
                                    "\n### 待回应的对话\n{}\n",
                                    deferred_ctx.join("\n")
                                );
                                memory_context.push_str(&deferred_section);
                            }
                            // 标记已消费
                            handler.cleanup_processed().await;
                        }
                    }
                    if !memory_context.is_empty() {
                        debug!("Memory context:\n{}", memory_context);
                    }

                    // 5. 三魂循环：人魂决策 → 天魂翻译 → 地魂审查 → 驳回则重试
                    // 循环直到审查通过或 deadline 到期
                    let max_retries = 3; // 防止无限循环
                    let agent_id = world_state.agent_id.unwrap_or_default();
                    let mut final_intent = None;

                    // 计算总 deadline（留 3s 缓冲给网络发送）
                    let deadline_at = if world_state.deadline_ms > 0 {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let remaining = world_state.deadline_ms.saturating_sub(now_ms);
                        Some(std::time::Instant::now() + std::time::Duration::from_millis(remaining.saturating_sub(3_000)))
                    } else {
                        None
                    };

                    for attempt in 0..=max_retries {
                        // 检查 deadline（Tick 关单）
                        if let Some(dl) = deadline_at
                            && std::time::Instant::now() >= dl {
                                warn!("Tick {} 三魂循环被 Tick 关单打断，第 {} 次尝试", world_state.tick_id, attempt);
                                // 不发送 intent（server 已关单），保留 rejection reason 供下个 tick
                                break;
                            }

                        // 5a. 人魂 (ActorSoul) 决策 — 输出叙事意图
                        // 优先使用 decision_with_chain_callback（返回 CognitiveChain 供天魂使用）
                        let (raw_intent, cognitive_chain) = {
                            let decision_future = async {
                                // 最高优先级：decision_with_chain_callback（支持天魂翻译）
                                if let Some(ref chain_callback) = self.decision_with_chain_callback {
                                    // 有 rejection reason 时传递 feedback
                                    let fb = self.last_rejection_reason.as_deref();
                                    return chain_callback(&world_state, &memory_context, fb).await;
                                }

                                // 有 rejection reason 时走 feedback callback（使用 [验证反馈] section）
                                // feedback callback 内部有 CognitiveValidator 重试
                                if let Some(ref reason) = self.last_rejection_reason {
                                    if let Some(ref callback) = self.decision_with_feedback_callback {
                                        let intent = callback(&world_state, &memory_context, Some(reason.as_str())).await;
                                        (intent, None)
                                    } else if let Some(ref memory_callback) = self.decision_with_memory_callback {
                                        // fallback: 将 rejection 混入 memory context
                                        let combined = if memory_context.is_empty() {
                                            format!("[意图被驳回: {}，请重新决策]", reason)
                                        } else {
                                            format!("{}\n[意图被驳回: {}，请重新决策]", memory_context, reason)
                                        };
                                        let intent = memory_callback(&world_state, &combined).await;
                                        (intent, None)
                                    } else {
                                        let intent = (self.decision_callback)(&world_state).await;
                                        (intent, None)
                                    }
                                } else if let Some(ref memory_callback) = self.decision_with_memory_callback {
                                    let intent = memory_callback(&world_state, &memory_context).await;
                                    (intent, None)
                                } else {
                                    let intent = (self.decision_callback)(&world_state).await;
                                    (intent, None)
                                }
                            };

                            if let Some(dl) = deadline_at {
                                let remaining = dl.saturating_duration_since(std::time::Instant::now());
                                match tokio::time::timeout(remaining, decision_future).await {
                                    Ok(result) => result,
                                    Err(_) => {
                                        warn!("Tick {} 第 {} 次人魂推理超时，放弃本轮", world_state.tick_id, attempt);
                                        // 不发送 intent（server 可能已关单）
                                        break;
                                    }
                                }
                            } else {
                                decision_future.await
                            }
                        };

                        // 如果 final_intent 已被设为超时 idle，退出
                        // 如果 final_intent 已被设为超时 idle，退出
                        if final_intent.is_some() { break; }

                        // 记录人魂输出
                        let renhun_narrative = raw_intent.action_data
                            .as_ref()
                            .and_then(|d| d.get("narrative"))
                            .and_then(|n| n.as_str())
                            .unwrap_or("");
                        let renhun_thought_log = raw_intent.thought_log.as_deref().unwrap_or("");
                        if let Some(recorder) = self.soul_recorder().await {
                            recorder.record_renhun(
                                world_state.tick_id,
                                attempt,
                                renhun_narrative,
                                renhun_thought_log,
                            ).await;
                            // 记录游戏内时间和现实时间
                            let world_time_str = Self::format_world_time(&world_state.world_time);
                            recorder.record_world_time(world_state.tick_id, attempt, &world_time_str).await;
                        }

                        // 5b. 天魂 (IntentTranslator) 翻译 — 叙事→格式化
                        let translation = self.translate_intent(raw_intent, &world_state, cognitive_chain.as_ref()).await;

                        // 记录天魂翻译结果
                        if let Some(recorder) = self.soul_recorder().await {
                            let action_data_str = translation.intent.action_data.as_ref()
                                .map(|d| serde_json::to_string(d).unwrap_or_default());
                            recorder.record_tianhun(
                                world_state.tick_id,
                                attempt,
                                Some(translation.intent.action_type.as_str()),
                                action_data_str.as_deref(),
                                translation.speech_intent.as_ref().and_then(|s| {
                                    s.action_data.as_ref()?.get("content")?.as_str()
                                }),
                                translation.success,
                                translation.error.as_deref(),
                            ).await;
                        }

                        // 5b'. 如果天魂拆分出说话 intent，走即时通道
                        if let Some(speech) = &translation.speech_intent {
                            let status = self.send_immediate_intent(speech).await;
                            if let Some(recorder) = self.soul_recorder().await {
                                recorder.record_immediate(
                                    world_state.tick_id,
                                    &speech.intent_id.to_string(),
                                    Some(&translation.original_narrative),
                                    "extracted",
                                    speech.action_type.as_str(),
                                    speech.action_data.as_ref().map(|d| serde_json::to_string(d).unwrap_or_default()).as_deref(),
                                    speech.action_data.as_ref().and_then(|d| d.get("content")?.as_str()),
                                    if status.is_ok() { "sent" } else { "failed" },
                                    status.err().map(|e| e.to_string()).as_deref(),
                                ).await;
                            }
                        }

                        let intent = translation.intent;

                        // 5c. 地魂 (ReflectorSoul) 审查 — 三层验证
                        match self.validate_with_reflector(intent, &world_state).await? {
                            super::agent::ReflectorResult::Approved { intent: approved_intent, layers, narrative } => {
                                // 记录地魂审查结果
                                if let Some(recorder) = self.soul_recorder().await {
                                    let layer1 = layers.iter().find(|l| l.layer == "layer1");
                                    let layer2 = layers.iter().find(|l| l.layer == "layer2");
                                    let layer3 = layers.iter().find(|l| l.layer == "layer3");
                                    recorder.record_dihun(
                                        world_state.tick_id,
                                        attempt,
                                        "approved",
                                        layer1.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                        layer2.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                        layer3.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                        None,
                                        narrative.as_deref(),
                                    ).await;
                                    recorder.record_final_intent(
                                        world_state.tick_id,
                                        attempt,
                                        Some(&approved_intent.intent_id.to_string()),
                                        Some(approved_intent.action_type.as_str()),
                                        approved_intent.action_data.as_ref().map(|d| serde_json::to_string(d).unwrap_or_default()).as_deref(),
                                    ).await;
                                }
                                final_intent = Some(approved_intent);
                                break;
                            }
                            super::agent::ReflectorResult::Rejected { reason, layers } => {
                                warn!("Tick {} 第 {} 次地魂审查驳回: {}", world_state.tick_id, attempt, reason);
                                // 叙事化驳回原因：人魂不应看到技术性 meta 信息（item_id 等）
                                let narrated_reason = super::Agent::narrativize_rejection(&reason);
                                self.last_rejection_reason =
                                    Some(narrated_reason.clone());

                                // 记录地魂审查结果
                                if let Some(recorder) = self.soul_recorder().await {
                                    let layer1 = layers.iter().find(|l| l.layer == "layer1");
                                    let layer2 = layers.iter().find(|l| l.layer == "layer2");
                                    let layer3 = layers.iter().find(|l| l.layer == "layer3");
                                    recorder.record_dihun(
                                        world_state.tick_id,
                                        attempt,
                                        "rejected",
                                        layer1.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                        layer2.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                        layer3.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                        Some(&reason),
                                        Some(&narrated_reason),
                                    ).await;
                                }

                                if attempt >= max_retries {
                                    warn!("Tick {} 达到最大重试次数 {}，提交 idle", world_state.tick_id, max_retries);
                                    final_intent = Some(Intent::new(agent_id, world_state.tick_id, "idle", None)
                                        .with_thought(format!("意图多次被驳回: {}", reason)));
                                    break;
                                }
                                // last_rejection_reason 已在此处叙事化设置
                                // 下一轮人魂会看到叙事化的 rejection reason（不含 meta 信息）
                            }
                        }
                    }

                    let final_intent = match final_intent {
                        Some(intent) => intent,
                        None => {
                            warn!("Tick {} 无有效 intent（超时或被驳回耗尽），发送 idle", world_state.tick_id);
                            self.consecutive_idle_count += 1;
                            self.maybe_rotate_model().await;
                            // 构造 idle intent 并继续发送+上报（保证 server-web 经历日志完整）
                            Intent::new(agent_id, world_state.tick_id, "idle", None)
                                .with_thought("三魂循环未产出有效意图".to_string())
                        }
                    };

                    // idle intent 也计入连续 idle
                    if final_intent.action_type.as_str() == "idle" {
                        self.consecutive_idle_count += 1;
                    }

                    // 5.6 记录 Intent 到经历日志（供 Web Panel 查询）
                    if let Some(ref api_state) = self.http_api_state
                        && let Some(history) = api_state.intent_history.read().await.as_ref() {
                            history
                                .record_intent(
                                    final_intent.tick_id,
                                    final_intent.intent_id,
                                    final_intent.action_type.to_string(),
                                    final_intent.thought_log.clone(),
                                )
                                .await;
                        }

                    // 6. 更新寿命状态（如果启用）
                    if let Some(ref mut calculator) = self.lifespan_calculator {
                        let status = calculator.process_tick();
                        if status.is_deceased() {
                            info!(
                                "Agent '{}' has passed away at age {}",
                                self.character_name(),
                                status.age()
                            );
                            // 发送最后一个 idle 意图后退出
                            let agent_id = self.client.agent_id().await.unwrap_or_default();
                            self.client
                                .send_intent(&Intent::new(
                                    agent_id,
                                    world_state.tick_id,
                                    "idle",
                                    None,
                                ))
                                .await
                                .ok();
                            return Ok(());
                        }
                    }

                    // 7. 发送意图
                    // 设置当前意图类型，让即时事件处理器进行冲突检测
                    if let Some(ref handler) = self.immediate_handler {
                        handler.set_current_intent(Some(final_intent.action_type.to_string())).await;
                    }

                    if let Err(e) = self.client.send_intent(&final_intent).await {
                        error!("Failed to send intent: {}", e);
                        // 清除当前意图类型
                        if let Some(ref handler) = self.immediate_handler {
                            handler.set_current_intent(None).await;
                        }
                        // send_intent 是 fire-and-forget，tick mismatch 由 receive 路径处理
                        // 此处只需重连
                        if let Err(reconnect_err) = self.reconnect().await {
                            error!("Reconnect failed: {}", reconnect_err);
                        }
                    } else {
                        info!(
                            "Intent sent successfully: tick={}, action={}, agent={}",
                            final_intent.tick_id, final_intent.action_type, final_intent.agent_id
                        );
                        // 记录 action_type 用于行为多样性检测
                        if let Some(ref engine) = self.cognitive_engine {
                            engine.record_action(&final_intent.action_type);
                        }
                        // 非 idle 成功发送，重置连续 idle 计数
                        if final_intent.action_type.as_str() != "idle" {
                            self.consecutive_idle_count = 0;
                        }
                        // idle 发送成功后检查是否需要 rotate
                        if final_intent.action_type.as_str() == "idle" {
                            self.maybe_rotate_model().await;
                        }
                        // 清除当前意图类型
                        if let Some(ref handler) = self.immediate_handler {
                            handler.set_current_intent(None).await;
                        }

                        // 7.5 上报三魂循环元数据到服务器（使 server-web 可见）
                        let tick_id_for_report = final_intent.tick_id;
                        if let Some(recorder) = self.soul_recorder().await {
                            let records = recorder.get_by_tick(tick_id_for_report).await;
                            let immediate_records = recorder.get_immediate_by_tick(tick_id_for_report).await;

                            // 从第一条记录获取游戏内时间
                            let world_time = records.first().and_then(|r| r.world_time.clone());

                            let cycles: Vec<cyber_jianghu_protocol::SoulCycleAttempt> = records.into_iter().map(|r| {
                                let layers: Vec<cyber_jianghu_protocol::LayerReport> = vec![
                                    (r.dihun_layer1_result.as_deref(), "layer1"),
                                    (r.dihun_layer2_result.as_deref(), "layer2"),
                                    (r.dihun_layer3_result.as_deref(), "layer3"),
                                ].into_iter().filter_map(|(detail, layer)| {
                                    detail.map(|d| cyber_jianghu_protocol::LayerReport {
                                        layer: layer.to_string(),
                                        passed: d == "通过" || d.is_empty(),
                                        detail: if d == "通过" || d.is_empty() { None } else { Some(d.to_string()) },
                                    })
                                }).collect();

                                cyber_jianghu_protocol::SoulCycleAttempt {
                                    attempt: r.attempt,
                                    renhun: cyber_jianghu_protocol::RenhunReport {
                                        narrative: r.renhun_narrative,
                                        thought_log: r.renhun_thought_log,
                                    },
                                    tianhun: cyber_jianghu_protocol::TianhunReport {
                                        action_type: r.tianhun_action_type,
                                        action_data: r.tianhun_action_data.as_ref().and_then(|s| serde_json::from_str(s).ok()),
                                        speech_content: r.tianhun_speech_content,
                                        success: r.tianhun_success,
                                        error: r.tianhun_error,
                                    },
                                    dihun: cyber_jianghu_protocol::DihunReport {
                                        result: r.dihun_result,
                                        layers,
                                        reason: r.dihun_reason,
                                        narrative: r.dihun_narrative,
                                    },
                                    final_intent: r.final_intent_id.map(|id| cyber_jianghu_protocol::FinalIntentReport {
                                        intent_id: Some(id),
                                        action_type: r.final_action_type.clone(),
                                        action_data: r.final_action_data.as_ref().and_then(|s| serde_json::from_str(s).ok()),
                                    }),
                                }
                            }).collect();

                            let immediate_intents: Vec<cyber_jianghu_protocol::ImmediateIntentReport> = immediate_records.into_iter().map(|r| {
                                cyber_jianghu_protocol::ImmediateIntentReport {
                                    intent_id: r.intent_id,
                                    route_type: r.route_type,
                                    action_type: r.action_type,
                                    action_data: r.action_data.as_ref().and_then(|s| serde_json::from_str(s).ok()),
                                    speech_content: r.speech_content,
                                    send_status: r.send_status,
                                    send_error: r.send_error,
                                }
                            }).collect();

                            let metadata = cyber_jianghu_protocol::SoulCycleMetadata {
                                world_time,
                                cycles,
                                immediate_intents,
                            };

                            // Fire-and-forget：上报失败不影响主循环
                            self.client.send_soul_cycle_report(tick_id_for_report, metadata).await.ok();
                        }
                    }
                }
            }
        }
    }

    /// 发送即时 Intent（通过 immediate_msg_tx，不走 intent 配额）
    ///
    /// 天魂路由出的 speak/whisper 或混合说话走此通道，
    /// 与 ImmediateEventHandler 的 RespondNow 共享同一条 WebSocket channel。
    async fn send_immediate_intent(&self, intent: &Intent) -> std::result::Result<(), String> {
        use cyber_jianghu_protocol::ClientMessage;

        let msg = ClientMessage::Intent {
            intent_id: Some(intent.intent_id),
            tick_id: intent.tick_id,
            agent_id: Some(intent.agent_id),
            thought_log: intent.thought_log.clone(),
            action_type: intent.action_type.to_string(),
            action_data: intent.action_data.clone(),
            priority: 10, // 即时高优先级
        };

        if let Err(e) = self.client.send_immediate_message(msg).await {
            warn!(
                "[天魂] 即时 intent 发送失败 ({}): {}",
                intent.action_type, e
            );
            Err(e.to_string())
        } else {
            info!(
                "[天魂] 即时 intent 已发送: {} {:?}",
                intent.action_type, intent.action_data
            );
            Ok(())
        }
    }

    /// 关闭连接
    pub async fn close(&mut self) -> Result<()> {
        self.client.close().await;
        info!("Agent '{}' stopped", self.character_name());
        Ok(())
    }

    /// 格式化游戏内时间（WorldTime → 中文武侠风格字符串）
    fn format_world_time(wt: &WorldTime) -> String {
        wt.to_chinese()
    }
}
