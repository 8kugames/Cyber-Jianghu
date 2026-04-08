// ============================================================================
// 连接生命周期管理
// ============================================================================
//
// 处理 Agent 的连接、主循环和关闭
// 重连逻辑在 reconnect.rs 中
// ============================================================================

use anyhow::Result;
use cyber_jianghu_protocol::ServerMessage;
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::component::llm::{DirectLlmClient, DirectLlmClientConfig, FallbackLlmClient, LlmProvider};
use crate::config::CharacterStatus;
use crate::infra::transport::ConnectError;
use crate::models::Intent;
use super::reconnect::{should_log_retry, save_character_config_to_fs};

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
            if let Some(ref engine) = self.cognitive_engine {
                engine.update_agent_name(name);
            }
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
                self.character_name(), agent_id
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

                    debug!("检测到配置变更，重新加载...");
                    let old_config_path = self.config.config_path.clone();
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
                                Ok(primary_client) => {
                                    // 构建 LLM 客户端（主模型 + fallback）
                                    let mut clients: Vec<std::sync::Arc<dyn crate::component::llm::LlmClient>> =
                                        vec![std::sync::Arc::new(primary_client)];
                                    // provider 已在上方解析并验证，复用而非重解析
                                    if let Some(resolved_provider) = provider {
                                        for fallback_model in &new_config.llm.fallback_models {
                                            let mut fb_config = DirectLlmClientConfig::new(
                                                resolved_provider,
                                                new_config.llm.api_key.clone(),
                                            );
                                            if let Some(ref url) = new_config.llm.base_url {
                                                fb_config = fb_config.with_base_url(url);
                                            }
                                            fb_config = fb_config
                                                .with_model(fallback_model)
                                                .with_temperature(new_config.llm.temperature)
                                                .with_max_tokens(new_config.llm.max_tokens);
                                            match DirectLlmClient::new(fb_config) {
                                                Ok(fb_client) => {
                                                    info!("热重载 fallback 模型: {}", fallback_model);
                                                    clients.push(std::sync::Arc::new(fb_client));
                                                }
                                                Err(e) => warn!("热重载 fallback 模型 {} 创建失败: {}", fallback_model, e),
                                            }
                                        }
                                    }

                                    let new_client: std::sync::Arc<dyn crate::component::llm::LlmClient> =
                                        if clients.len() > 1 {
                                            std::sync::Arc::new(FallbackLlmClient::new(clients))
                                        } else {
                                            clients.into_iter().next().unwrap()
                                        };

                                    // 先更新 config（保证一致性），再更新 container
                                    self.config = new_config;
                                    // 保留 config_path（from_file 反序列化时 #[serde(skip)] 会丢失）
                                    self.config.config_path = old_config_path;

                                    if let Some(ref container) = self.actor_llm_container {
                                        let mut guard = container.write().await;
                                        *guard = new_client;
                                        debug!("ActorSoul LLM 容器已更新（热重载 + fallback）");
                                    }

                                    debug!("ActorSoul LLM 已重载");
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

                    // 排空积压的重复通知（notify 在 macOS 上单次修改会触发多个事件）
                    if let Some(ref mut rx) = self.config_reload_rx {
                        while rx.try_recv().is_ok() {}
                    }
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

                    // 2. 处理事件并更新记忆
                    if let Err(e) = self.process_events(&world_state.events_log).await {
                        warn!("Failed to process events into memory: {}", e);
                    }

                    // 3. 每 FORGETTING_INTERVAL_TICKS tick 运行遗忘机制
                    if world_state.tick_id % super::FORGETTING_INTERVAL_TICKS == 0
                        && let Err(e) = self.run_forgetting(world_state.tick_id).await {
                            warn!("Failed to run forgetting mechanism: {}", e);
                        }

                    // 4. 构建增强的世界状态（包含记忆上下文）
                    let memory_context = self.get_memory_context().await;
                    if !memory_context.is_empty() {
                        debug!("Memory context:\n{}", memory_context);
                    }

                    // 5. 调用决策回调（带验证和记忆上下文）
                    // 用 deadline 计算剩余时间，超时自动 idle
                    let decision_future = async {
                        if let Some(ref memory_callback) = self.decision_with_memory_callback {
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
                        }
                    };

                    let intent = if world_state.deadline_ms > 0 {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let remaining_ms = world_state.deadline_ms.saturating_sub(now_ms);
                        // 留 3s 缓冲给 ReflectorSoul + 网络发送
                        let buffer_ms: u64 = 3_000;
                        let timeout_ms = remaining_ms.saturating_sub(buffer_ms);
                        let decision_timeout = std::time::Duration::from_millis(timeout_ms);

                        match tokio::time::timeout(decision_timeout, decision_future).await {
                            Ok(intent) => intent,
                            Err(_) => {
                                warn!(
                                    "Tick {} LLM 推理超时（剩余 {:?}，限制 {:?}），自动 idle",
                                    world_state.tick_id,
                                    std::time::Duration::from_millis(remaining_ms),
                                    decision_timeout,
                                );
                                let agent_id = world_state.agent_id.unwrap_or_default();
                                Intent::new(agent_id, world_state.tick_id, "idle", None)
                                    .with_thought("忽然心神恍惚，思绪难聚，只得静坐调息片刻".to_string())
                            }
                        }
                    } else {
                        // deadline_ms == 0：无时间限制，直接决策
                        decision_future.await
                    };

                    // 5.5 ReflectorSoul 同步审查（反思之魂）
                    let final_intent = self.validate_with_reflector(intent, &world_state).await?;

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

    /// 关闭连接
    pub async fn close(&mut self) -> Result<()> {
        self.client.close().await;
        info!("Agent '{}' stopped", self.character_name());
        Ok(())
    }
}
