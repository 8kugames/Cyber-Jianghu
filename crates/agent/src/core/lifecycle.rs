// ============================================================================
// 连接生命周期管理
// ============================================================================
//
// 处理 Agent 的连接、重连、主循环和关闭
// ============================================================================

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::ai::llm::{DirectLlmClient, DirectLlmClientConfig, LlmProvider};
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

        // 等待注册确认（包含游戏规则）
        let (agent_id, game_rules) = self.client.wait_for_registration().await?;
        info!("Agent '{}' registered with server", self.character_name());
        info!("Server-assigned Agent ID: {}", agent_id);

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
                            let provider = LlmProvider::from_str(&new_config.llm.provider);
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

                    // 1.5 检查是否死亡（只报告一次）
                    if !self.death_reported {
                        if let Some(death_event) = world_state.events_log.iter().find(|e| {
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
                            // 可以在这里处理死亡逻辑，例如退出循环或进入观察者模式
                            // 目前 MVP 阶段，我们只是记录日志并继续（可能会尝试发送 idle 直到被踢出）
                        }
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
                                .send_intent(&Intent::idle(
                                    agent_id,
                                    world_state.tick_id,
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
                        Ok((agent_id, game_rules)) => {
                            info!("重连后注册确认: agent_id={}", agent_id);

                            // 调用注册回调（更新外部状态如 HTTP API 的 agent_id）
                            if let Some(ref callback) = self.registration_callback {
                                callback(agent_id);
                            }

                            // 更新游戏规则
                            self.config.update_game_rules(game_rules);
                        }
                        Err(e) => {
                            // 注册确认失败，可能需要重新建立连接
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
