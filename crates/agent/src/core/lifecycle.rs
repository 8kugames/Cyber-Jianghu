// ============================================================================
// 连接生命周期管理
// ============================================================================
//
// 处理 Agent 的连接、重连、主循环和关闭
// ============================================================================

use anyhow::Result;
use cyber_jianghu_protocol::ServerMessage;
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::component::llm::{DirectLlmClient, DirectLlmClientConfig, LlmProvider};
use crate::config::CharacterStatus;
use crate::infra::transport::ConnectError;
use crate::models::Intent;

/// 检查是否应该记录重试日志（日志采样策略）
///
/// 策略：
/// - 前 5 次：每次都记录
/// - 第 6 次后：仅当重试次数为完全平方数时记录（9, 16, 25, 36...）
fn should_log_retry(attempt: u32) -> bool {
    if attempt <= 5 {
        return true;
    }
    // 检查是否为完全平方数
    let sqrt = (attempt as f64).sqrt() as u32;
    sqrt * sqrt == attempt
}

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
                    "Agent '{}' status is {:?}, skipping server connection (waiting for rebirth)",
                    character.name, character.status
                );
            } else {
                warn!("No active character, skipping server connection");
            }
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
        // 如果收到 "Pending registration" 错误，说明需要创建角色，无限等待
        let (agent_id, game_rules) = loop {
            match self.client.wait_for_registration().await {
                Ok((id, rules)) => break (id, rules),
                Err(e) if format!("{}", e).contains("Pending registration") => {
                    self.reconnect_backoff += 1;
                    let elapsed_secs = self.reconnect_backoff * 5;
                    if should_log_retry(self.reconnect_backoff) {
                        info!(
                            "等待角色创建，Agent '{}' 将于 5 秒后重连... (已等待 {}秒)",
                            self.character_name(),
                            elapsed_secs
                        );
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    self.client.close().await;
                    // 重连可能失败，使用无限重试
                    loop {
                        match self.client.connect().await {
                            Ok(()) => break,
                            Err(ConnectError::AuthFailed) => {
                                match self.refresh_device_token().await {
                                    Ok(()) => continue,
                                    Err(token_err) => {
                                        warn!("Token 刷新失败 (等待角色创建期间): {}, 5秒后重试", token_err);
                                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                                    }
                                }
                            }
                            Err(ConnectError::ConnectionFailed(e)) => {
                                warn!("等待角色创建期间重连失败: {}, 5秒后重试...", e);
                                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                            }
                        }
                    }
                }
                Err(e) => return Err(e),
            }
        };
        // 重置重试计数器
        self.reconnect_backoff = 0;
        info!("Agent '{}' registered with server", self.character_name());
        info!("Server-assigned Agent ID: {}", agent_id);

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

            return Ok(());
        }

        // 调用注册回调（更新外部状态如 HTTP API 的 agent_id）
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

        loop {
            tokio::select! {
                // 检查重连请求（热切换）
                Some(req) = async {
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

                // 配置文件变更通知
                _ = async {
                    if let Some(ref mut rx) = self.config_reload_rx {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    // 防护：config_path 为空或文件不存在时跳过重载
                    let config_path = &self.config.config_path;
                    if config_path.as_os_str().is_empty() || !config_path.exists() {
                        debug!("配置路径无效，跳过重载: {:?}", config_path);
                        if let Some(ref mut rx) = self.config_reload_rx {
                            while rx.try_recv().is_ok() {}
                        }
                        continue;
                    }

                    info!("检测到配置变更，重新加载...");
                    let old_config = self.config.clone();

                    match crate::config::Config::from_file(config_path) {
                        Ok(new_config) => {
                            // 创建新的 LLM 客户端
                            let provider = LlmProvider::parse(&new_config.llm.provider);
                            let llm_client_result = match provider {
                                Some(provider) => {
                                    let mut client_config = DirectLlmClientConfig::new(
                                        provider,
                                        new_config.llm.api_key.clone(),
                                    );
                                    if let Some(ref url) = new_config.llm.base_url {
                                        client_config = client_config.with_base_url(url);
                                    }
                                    if let Some(ref model) = new_config.llm.model {
                                        client_config = client_config.with_model(model);
                                    }
                                    client_config = client_config
                                        .with_temperature(new_config.llm.temperature)
                                        .with_max_tokens(new_config.llm.max_tokens);
                                    DirectLlmClient::new(client_config)
                                }
                                None => {
                                    Err(anyhow::anyhow!("Unknown LLM provider: {}", new_config.llm.provider))
                                }
                            };

                            match llm_client_result {
                                Ok(client) => {
                                    let new_client = std::sync::Arc::new(client);

                                    // 更新 actor_llm_container（热重载）
                                    // 决策回调会自动使用新的 LLM Client
                                    if let Some(ref container) = self.actor_llm_container {
                                        let mut guard = container.write().await;
                                        *guard = new_client.clone();
                                        info!("ActorSoul LLM 容器已更新（真正热重载）");
                                    }

                                    self.config = new_config;
                                    info!("ActorSoul LLM 已重载");
                                }
                                Err(e) => {
                                    warn!("ActorSoul LLM 重载失败: {}，保持旧配置", e);
                                    self.config = old_config;
                                }
                            }
                        }
                        Err(e) => {
                            warn!("配置读取失败: {}，保持旧配置", e);
                        }
                    }
                }

                // 接收世界状态
                result = self.client.receive_world_state() => {
                    let world_state = match result {
                        Ok(state) => state,
                        Err(e) => {
                            let error_msg = format!("{}", e);
                            // websocket.rs 已将 tick mismatch 转为 "Tick mismatch: Intent tick_id X 不匹配当前 tick Y..."
                            if error_msg.starts_with("Tick mismatch") {
                                // 从 "Intent tick_id X 不匹配当前 tick Y" 中提取服务端 tick
                                let current_server_tick: Option<i64> = error_msg
                                    .split("当前 tick ")
                                    .nth(1)
                                    .and_then(|s| s.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok());
                                
                                info!("Tick mismatch detected: {}. Server tick: {:?}. Reconnecting...", e, current_server_tick);
                                
                                // 重连
                                if let Err(reconnect_err) = self.reconnect().await {
                                    error!("Reconnect failed: {}", reconnect_err);
                                    continue;
                                }
                                
                                // 接收新的 WorldState（可能是过时的初始 WorldState）
                                let new_world_state = match self.client.receive_world_state().await {
                                    Ok(ws) => {
                                        info!("Received WorldState after reconnect: tick={}", ws.tick_id);
                                        ws
                                    }
                                    Err(ws_err) => {
                                        error!("Failed to receive WorldState after reconnect: {}", ws_err);
                                        continue;
                                    }
                                };
                                
                                // 如果有 current_server_tick 且收到的 WorldState tick 太旧，
                                // 直接用 server_tick 生成 idle intent，不要等待（会死锁）
                                let intent_tick = if let Some(server_tick) = current_server_tick {
                                    if new_world_state.tick_id < server_tick {
                                        info!("WorldState tick {} < server tick {}, using server tick {} for intent",
                                            new_world_state.tick_id, server_tick, server_tick);
                                        server_tick
                                    } else {
                                        new_world_state.tick_id
                                    }
                                } else {
                                    new_world_state.tick_id
                                };
                                
                                // 更新 HTTP API 状态
                                if let Some(ref api_state) = self.http_api_state {
                                    let mut current = api_state.current_state.write().await;
                                    *current = Some(new_world_state.clone());
                                    let mut last_update = api_state.last_state_update.write().await;
                                    *last_update = Some(std::time::Instant::now());
                                }
                                
                                // 用新 WorldState 重新生成 intent
                                let memory_context = self.get_memory_context().await;
                                let combined_context = match &self.last_rejection_reason {
                                    Some(reason) => {
                                        if memory_context.is_empty() {
                                            format!("[上次意图被驳回: {}]", reason)
                                        } else {
                                            format!("{}\n[上次意图被驳回: {}]", memory_context, reason)
                                        }
                                    }
                                    None => memory_context.clone(),
                                };
                                
                                let intent = if let Some(ref memory_callback) = self.decision_with_memory_callback {
                                    memory_callback(&new_world_state, &combined_context).await
                                } else if let Some(ref reason) = self.last_rejection_reason {
                                    if let Some(ref callback) = self.decision_with_feedback_callback {
                                        callback(&new_world_state, Some(reason.as_str())).await
                                    } else {
                                        (self.decision_callback)(&new_world_state).await
                                    }
                                } else {
                                    (self.decision_callback)(&new_world_state).await
                                };
                                
                                // 重新验证
                                let mut final_intent = match self.validate_with_reflector(intent, &new_world_state).await {
                                    Ok(validated) => validated,
                                    Err(ref validation_err) => {
                                        error!("Re-validation failed after tick mismatch: {}", validation_err);
                                        continue;
                                    }
                                };
                                
                                // 如果使用了 server tick（落后于 WorldState），更新 intent 的 tick_id
                                if final_intent.tick_id != intent_tick {
                                    info!("Updating intent tick from {} to {}", final_intent.tick_id, intent_tick);
                                    final_intent.tick_id = intent_tick;
                                }
                                
                                // 重新发送
                                info!(
                                    "Resending intent after tick mismatch: tick={}, action={}",
                                    final_intent.tick_id, final_intent.action_type
                                );
                                if let Err(send_err) = self.client.send_intent(&final_intent).await {
                                    error!("Resend failed: {}", send_err);
                                } else {
                                    info!(
                                        "Intent resent successfully after tick mismatch: tick={}, action={}",
                                        final_intent.tick_id, final_intent.action_type
                                    );
                                }
                                
                                continue;
                            }
                            
                            // 其他错误，尝试重连并继续
                            error!("Failed to receive world state: {}", e);
                            self.reconnect().await?;
                            continue;
                        }
                    };

                    // 更新 HTTP API 状态（供 Web Panel 查询）
                    if let Some(ref api_state) = self.http_api_state {
                        let mut current = api_state.current_state.write().await;
                        *current = Some(world_state.clone());

                        let mut last_update = api_state.last_state_update.write().await;
                        *last_update = Some(std::time::Instant::now());

                        // 异步更新关系叙事（不阻塞）
                        api_state.maybe_update_narratives(&world_state).await;
                    }

                    // 1.5 检查是否死亡（只报告一次）
                    if !self.death_reported
                        && let Some(death_event) = world_state.events_log.iter().find(|e| {
                            if let Some(cause) = e.metadata.get("cause")
                                && let Some(cause_str) = cause.as_str() {
                                    return cause_str.starts_with("death");
                                }
                            if let Some(msg_type) = e.metadata.get("type")
                                && let Some(type_str) = msg_type.as_str() {
                                    return type_str == "death_notification";
                                }
                            false
                        }) {
                            warn!(
                                "Agent '{}' has died: {}",
                                self.character_name(), death_event.description
                            );
                            self.death_reported = true;
                            if let Some(ref api_state) = self.http_api_state {
                                api_state.is_dead.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            // 死亡后不退出，等待转生：
                            // - Cognitive 模式：继续循环，等待 rebirth handler 触发重连
                            // - Claw 模式：OpenClaw 已收到 AgentDied 信号，会通过 reconnect_rx 触发重连
                            continue;
                        }

                    // 2. 处理事件并更新记忆
                    if let Err(e) = self.process_events(&world_state.events_log).await {
                        warn!("Failed to process events into memory: {}", e);
                    }

                    // 3. 每 84 tick 运行遗忘机制
                    if world_state.tick_id % 84 == 0
                        && let Err(e) = self.run_forgetting(world_state.tick_id).await {
                            warn!("Failed to run forgetting mechanism: {}", e);
                        }

                    // 4. 构建增强的世界状态（包含记忆上下文）
                    let memory_context = self.get_memory_context().await;
                    if !memory_context.is_empty() {
                        debug!("Memory context:\n{}", memory_context);
                    }

                    // 5. 调用决策回调（带验证和记忆上下文）
                    let intent = if let Some(ref memory_callback) = self.decision_with_memory_callback {
                        // 带记忆上下文决策（记忆系统生效）
                        // 如果同时有 rejection feedback，也注入到 memory context 中
                        let combined_context = match &self.last_rejection_reason {
                            Some(reason) => {
                                if memory_context.is_empty() {
                                    format!("[上次意图被驳回: {}]", reason)
                                } else {
                                    format!("{}\n[上次意图被驳回: {}]", memory_context, reason)
                                }
                            }
                            None => memory_context.clone(),
                        };
                        memory_callback(&world_state, &combined_context).await
                    } else if let Some(ref reason) = self.last_rejection_reason {
                        // 有 rejection 但无 memory callback，走 feedback 回调
                        if let Some(ref callback) = self.decision_with_feedback_callback {
                            callback(&world_state, Some(reason.as_str())).await
                        } else {
                            (self.decision_callback)(&world_state).await
                        }
                    } else {
                        (self.decision_callback)(&world_state).await
                    };

                    // 5.5 ReflectorSoul 同步审查（反思之魂）
                    let final_intent = self.validate_with_reflector(intent, &world_state).await?;

                    // 5.6 记录 Intent 到经历日志（供 Web Panel 查询）
                    if let Some(ref api_state) = self.http_api_state
                        && let Some(ref history) = api_state.intent_history {
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
                    if let Err(e) = self.client.send_intent(&final_intent).await {
                        error!("Failed to send intent: {}", e);
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
                    }
                }
            }
        }
    }

    /// 重连服务端（无限重试，逐步降频策略）
    ///
    /// 降频策略：
    /// - 初始延迟 1 秒
    /// - 每次失败后延迟翻倍
    /// - 最大延迟为 tick_duration 的一半（确保每个 tick 至少尝试 2 次）
    /// - 重连成功后重置退避计数器
    async fn reconnect(&mut self) -> Result<()> {
        const INITIAL_DELAY_MS: u64 = 1000; // 1 秒

        // 获取 tick 时长，计算最大延迟（tick 的一半）
        let tick_duration_ms = self.get_tick_duration().await.as_millis() as u64;
        let max_delay_ms = tick_duration_ms / 2;

        self.client.close().await;

        loop {
            // 计算当前延迟：初始延迟 * 2^backoff，但不超过最大延迟
            let delay_ms = std::cmp::min(
                INITIAL_DELAY_MS * (1u64 << self.reconnect_backoff.min(10)),
                max_delay_ms,
            );

            let attempt = self.reconnect_backoff + 1;
            if should_log_retry(attempt) {
                warn!(
                    "重连尝试 {} (等待 {}ms, 最大 {}ms)...",
                    attempt, delay_ms, max_delay_ms
                );
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

            match self.client.connect().await {
                Ok(()) => {
                    info!("重连成功，尝试次数: {}", attempt);

                    // 等待 Server 发送 Registered 消息，获取最新的 agent_id 和 game_rules
                    match self.client.wait_for_registration().await {
                        Ok((agent_id, game_rules)) => {
                            info!("重连后注册确认: agent_id={}", agent_id);

                            // agent_id 为零 = 角色已归隐（可能在等待期间被删除）
                            if agent_id == Uuid::nil() {
                                warn!("重连后收到 nil agent_id，角色已归隐");
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
                                return Err(anyhow::anyhow!(
                                    "Pending registration: character retired"
                                ));
                            }

                            // 重置死亡状态（转生后获得新身份）
                            self.death_reported = false;
                            if let Some(ref api_state) = self.http_api_state {
                                api_state
                                    .is_dead
                                    .store(false, std::sync::atomic::Ordering::Relaxed);
                            }

                            // 调用注册回调（更新外部状态如 HTTP API 的 agent_id）
                            if let Some(ref callback) = self.registration_callback {
                                callback(agent_id);
                            }

                            // 更新游戏规则
                            self.config.update_game_rules(game_rules);
                        }
                        Err(e) => {
                            let err_msg = format!("{}", e);
                            // Pending registration = 需要注册新角色，停止重试
                            if err_msg.contains("Pending registration") {
                                info!("等待注册新角色，停止重连");
                                return Err(anyhow::anyhow!("Pending registration: {}", e));
                            }
                            // 其他错误，继续重试
                            error!("重连后注册确认失败: {}", e);
                            self.client.close().await;
                            // 增加退避计数器并继续重试
                            self.reconnect_backoff = self.reconnect_backoff.saturating_add(1);
                            continue;
                        }
                    }

                    // 重连成功，重置退避计数器
                    self.reconnect_backoff = 0;
                    return Ok(());
                }
                Err(ConnectError::AuthFailed) => {
                    warn!(
                        "重连 auth failed (attempt {}), refreshing token...",
                        attempt
                    );
                    match self.refresh_device_token().await {
                        Ok(()) => {
                            info!("Token refreshed, retrying reconnection...");
                            // 不增加退避计数器，因为 token 已刷新
                            continue;
                        }
                        Err(e) => {
                            if should_log_retry(attempt) {
                                warn!("重连 token refresh 失败 (attempt {}): {}", attempt, e);
                            }
                            // 增加退避计数器（逐步降低频率）
                            self.reconnect_backoff = self.reconnect_backoff.saturating_add(1);
                            // 继续循环，不退出
                        }
                    }
                }
                Err(ConnectError::ConnectionFailed(e)) => {
                    if should_log_retry(attempt) {
                        warn!("重连尝试 {} 失败: {}", attempt, e);
                    }
                    // 增加退避计数器（逐步降低频率）
                    self.reconnect_backoff = self.reconnect_backoff.saturating_add(1);
                    // 继续循环，不退出
                }
            }
        }
    }

    /// 刷新设备 token（WebSocket 400 认证失败时自动调用）
    ///
    /// 调用 `POST {server_http_url}/api/v1/agent/connect` 获取新的 auth_token，
    /// 然后更新客户端身份和本地 device_config。
    async fn refresh_device_token(&mut self) -> Result<()> {
        let device_id = self
            .device_config
            .as_ref()
            .map(|d| d.device_id)
            .ok_or_else(|| anyhow::anyhow!("No device_config, cannot refresh token"))?;

        let http_url = &self.config.server.http_url;
        let url = format!("{}/api/v1/agent/connect", http_url);

        debug!("Refreshing device token for {} at {}", device_id, url);

        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .json(&serde_json::json!({ "device_id": device_id }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Server returned error {}: {}", status, body);
        }

        #[derive(serde::Deserialize)]
        struct ConnectResponse {
            auth_token: String,
        }

        let result: ConnectResponse = response
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

        info!("Token refreshed successfully for device {}", device_id);

        // 更新客户端身份
        self.client
            .set_identity(device_id, result.auth_token.clone())
            .await;

        // 更新本地 device_config 并持久化
        if let Some(ref mut device) = self.device_config {
            device.auth_token = result.auth_token.clone();
            if let Err(e) = device.save_to_file(&self.config.device_yaml_path(&device.server_url)) {
                warn!("Failed to persist refreshed token: {}", e);
            }
        }

        Ok(())
    }

    /// 关闭连接
    pub async fn close(&mut self) -> Result<()> {
        self.client.close().await;
        info!("Agent '{}' stopped", self.character_name());
        Ok(())
    }
}
