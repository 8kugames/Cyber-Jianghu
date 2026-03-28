// ============================================================================
// 连接生命周期管理
// ============================================================================
//
// 处理 Agent 的连接、重连、主循环和关闭
// ============================================================================

use anyhow::{Context, Result};
use cyber_jianghu_protocol::ServerMessage;
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::ai::llm::{DirectLlmClient, DirectLlmClientConfig, LlmProvider};
use crate::config::CharacterStatus;
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

/// 向服务器注册设备身份
async fn register_device_identity(server_url: &str, device_id: Uuid) -> Result<String> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/agent/connect", server_url);

    info!("向服务器注册设备: {} -> {}", device_id, url);

    #[derive(serde::Serialize)]
    struct AgentConnectRequest {
        device_id: Uuid,
    }

    #[derive(serde::Deserialize)]
    struct AgentConnectResponse {
        auth_token: String,
        message: String,
    }

    let response = client
        .post(&url)
        .json(&AgentConnectRequest { device_id })
        .send()
        .await
        .context("Failed to connect to server")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Server returned error {}: {}", status, body);
    }

    let result: AgentConnectResponse = response
        .json()
        .await
        .context("Failed to parse server response")?;

    info!("服务器响应: {}", result.message);
    Ok(result.auth_token)
}

impl super::Agent {
    /// 运行 Agent 主循环
    ///
    /// 持续接收世界状态，做出决策，发送意图
    pub async fn run(&mut self) -> Result<()> {
        // 检查角色状态：若已死亡或已归隐，跳过服务器连接
        let skip_connection = self.death_reported
            || self
                .config
                .agent
                .as_ref()
                .map(|c| c.status != CharacterStatus::Alive)
                .unwrap_or(false);

        if skip_connection {
            if let Some(ref agent) = self.config.agent {
                warn!(
                    "Agent '{}' status is {:?}, skipping server connection (waiting for rebirth)",
                    agent.name, agent.status
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
                Err(e) => {
                    if should_log_retry(connect_attempt) {
                        warn!(
                            "连接游戏服务器失败 (尝试 {}): {}, 5秒后重试...",
                            connect_attempt, e
                        );
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            }
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

        // 等待注册确认（包含游戏规则和存活状态）
        let registration_result = self.client.wait_for_registration().await;

        // 处理 "Invalid device credentials" 错误：清除身份并重新注册
        if let Err(ref e) = registration_result {
            let err_msg = e.to_string();
            if err_msg.contains("Invalid device credentials") {
                warn!("服务器拒绝设备凭证，将清除旧身份并重新注册...");
                self.config.clear_identity();
                // 重新连接并注册
                self.client.close().await;
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                // 生成新身份并注册
                let new_device_id = Uuid::new_v4();
                let http_url = self.config.server.http_url.clone();
                match register_device_identity(&http_url, new_device_id).await {
                    Ok(auth_token) => {
                        self.config.identity = Some(crate::config::IdentityConfig {
                            device_id: new_device_id,
                            auth_token,
                            server_url: Some(http_url),
                        });
                        if let Err(save_err) = self.config.save_to_file(&self.config.config_path) {
                            warn!("保存新身份失败: {}", save_err);
                        }
                        info!("新身份已注册: {}", new_device_id);
                    }
                    Err(reg_err) => {
                        error!("重新注册失败: {}", reg_err);
                    }
                }
            }
        }

        let (agent_id, game_rules, is_alive) = registration_result?;
        info!("Agent '{}' registered with server", self.character_name());
        info!(
            "Server-assigned Agent ID: {} (alive={})",
            agent_id, is_alive
        );

        // is_alive == false = 角色已死亡/归隐/未创建，跳过主循环，等待转生/创建角色
        // 覆盖两种情况：agent_id 为 nil（已归隐）或 agent_id 非 nil 但已死亡（竞态窗口）
        if !is_alive {
            warn!(
                "Agent '{}' is not alive (agent_id={})",
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
                    cause: "retired".to_string(),
                    description: "角色已归隐或尚未创建，请创建新角色".to_string(),
                    location: String::new(),
                    tick_id: 0,
                    died_at: chrono::Utc::now().timestamp_millis(),
                    rebirth_delay_ticks: 0,
                };
                let _ = api_state.death_event_tx.send(death_msg);
            }

            // 不退出！等待配置重载（角色创建会触发配置重载）后重新连接
            info!(
                "Agent '{}' 等待角色创建，HTTP API 保持运行...",
                self.character_name()
            );

            loop {
                tokio::select! {
                    // 等待配置重载（角色创建会触发配置重载）
                    _ = async {
                        if let Some(ref mut rx) = self.config_reload_rx {
                            rx.recv().await
                        } else {
                            std::future::pending().await
                        }
                    } => {
                        info!("检测到配置变更，重新加载...");
                        match crate::config::Config::from_file(&self.config.config_path) {
                            Ok(new_config) => {
                                self.config = new_config;
                                info!("配置已重载，尝试重新连接...");
                            }
                            Err(e) => {
                                warn!("配置读取失败: {}，继续等待", e);
                                continue;
                            }
                        }
                    }
                    // 等待重连请求（转生/角色切换）
                    Some(req) = async {
                        if let Some(ref mut rx) = self.reconnect_rx {
                            rx.recv().await
                        } else {
                            std::future::pending().await
                        }
                    } => {
                        info!("[main] 收到重连请求: {}", req.ws_url);
                        let http_url = req.ws_url
                            .replace("ws://", "http://")
                            .replace("wss://", "https://")
                            .replace("/ws", "");
                        self.client.update_server_url(req.ws_url.clone(), http_url).await;
                        break; // 跳出循环，执行重连
                    }
                    // 定期检查配置是否有角色信息
                    _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {
                        info!("定期检查配置是否有角色创建...");
                        match crate::config::Config::from_file(&self.config.config_path) {
                            Ok(new_config) => {
                                let had_character_before = self.config.agent.is_some();
                                let has_character_now = new_config.agent.is_some();
                                // 检查是否有新角色被创建
                                if !had_character_before && has_character_now {
                                    info!("检测到新角色，重新连接...");
                                    self.config = new_config;
                                    break;
                                }
                                self.config = new_config;
                            }
                            Err(e) => {
                                debug!("配置读取失败（定期检查）: {}", e);
                            }
                        }
                    }
                }

                // 重置状态，准备重新连接
                self.death_reported = false;
                if let Some(ref api_state) = self.http_api_state {
                    api_state.is_dead.store(false, std::sync::atomic::Ordering::Relaxed);
                }
                self.client.close().await;

                // 重新连接并注册
                match self.client.connect().await {
                    Ok(()) => info!("重新连接成功"),
                    Err(e) => {
                        warn!("重新连接失败: {}，5秒后重试...", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        continue;
                    }
                }

                match self.client.wait_for_registration().await {
                    Ok((new_agent_id, new_game_rules, new_is_alive)) => {
                        info!(
                            "重新注册成功: agent_id={} (alive={})",
                            new_agent_id, new_is_alive
                        );

                        if new_is_alive {
                            // 角色已创建，更新状态并进入主循环
                            if let Some(ref callback) = self.registration_callback {
                                callback(new_agent_id);
                            }
                            self.config.update_game_rules(new_game_rules);
                            // 重置死亡标记
                            self.death_reported = false;
                            if let Some(ref api_state) = self.http_api_state {
                                api_state.is_dead.store(false, std::sync::atomic::Ordering::Relaxed);
                            }
                            // 继续执行到下面的主循环
                            info!("角色已就绪，进入游戏主循环");
                            break; // 退出等待循环，继续到主循环
                        } else {
                            // 仍然不存活，继续等待
                            info!("角色仍未创建，继续等待...");
                            self.death_reported = true;
                            if let Some(ref api_state) = self.http_api_state {
                                api_state.is_dead.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("重新注册失败: {}，继续等待...", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                }
            }

            // 如果到这里，说明角色已创建（alive=true），继续执行到主循环
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
                    let http_url = req.ws_url
                        .replace("ws://", "http://")
                        .replace("wss://", "https://")
                        .replace("/ws", "");
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
                    info!("检测到配置变更，重新加载...");
                    let old_config = self.config.clone();

                    match crate::config::Config::from_file(&self.config.config_path) {
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

                                    // 更新 actor_llm_client（向后兼容）
                                    self.actor_llm_client = Some(new_client.clone());

                                    // 更新 actor_llm_container（真正热重载）
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
                            error!("Failed to receive world state: {}", e);
                            // 尝试重连
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

                    // 4.5 检查 WebSocket 层面的死亡通知（is_dead 标志由 AgentDied 消息设置）
                    // 避免 events_log 检测与 AgentDied 消息之间的时序竞争
                    if let Some(ref api_state) = self.http_api_state
                        && api_state.is_dead.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        if !self.death_reported {
                            warn!(
                                "Agent '{}' detected dead via is_dead flag (WebSocket AgentDied), skipping decision",
                                self.character_name()
                            );
                            self.death_reported = true;
                        }
                        continue;
                    }

                    // 5. 调用决策回调（带验证和记忆上下文）
                    let intent = if self.validator.is_some() {
                        self.decide_with_validation(&world_state).await?
                    } else if let Some(ref memory_callback) = self.decision_with_memory_callback {
                        // 优先使用带记忆上下文的回调
                        memory_callback(&world_state, &memory_context).await
                    } else {
                        (self.decision_callback)(&world_state).await
                    };

                    // 5.5 审查（ReflectorSoul - 反思之魂）
                    let final_intent = if let Some(ref store) = self.review_store {
                        self.submit_for_review(intent, &world_state, store).await?
                    } else {
                        intent
                    };

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
                        // 尝试重连
                        self.reconnect().await?;
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
                        Ok((agent_id, game_rules, is_alive)) => {
                            info!("重连后注册确认: agent_id={} (alive={})", agent_id, is_alive);

                            // is_alive == false = 角色已死亡/归隐（可能在等待期间被删除）
                            if !is_alive {
                                warn!("重连后角色不存活 (agent_id={})", agent_id);
                                self.death_reported = true;
                                if let Some(ref api_state) = self.http_api_state {
                                    api_state
                                        .is_dead
                                        .store(true, std::sync::atomic::Ordering::Relaxed);
                                    let death_msg = ServerMessage::AgentDied {
                                        agent_id,
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
                Err(e) => {
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

    /// 关闭连接
    pub async fn close(&mut self) -> Result<()> {
        self.client.close().await;
        info!("Agent '{}' stopped", self.character_name());
        Ok(())
    }
}
