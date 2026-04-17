// ============================================================================
// 连接生命周期管理
// ============================================================================
//
// 处理 Agent 的连接、主循环和关闭
// 重连逻辑在 reconnect.rs 中
// ============================================================================

use anyhow::Result;
use cyber_jianghu_protocol::{ExecutionSummary, ServerMessage, WorldTime};
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

        // 注入 HTTP API 状态到 ImmediateEventHandler（用于记录即时意图到 SoulRecorder）
        if let (Some(handler), Some(api_state)) = (&self.immediate_handler, &self.http_api_state) {
            handler.set_http_api_state(api_state.clone()).await;
        }

        // 设置游戏规则更新回调
        let agent_name_for_callback = self.character_name().to_string();
        let immediate_handler_for_rules = self.immediate_handler.clone();
        let llm_container_for_rules = self.actor_llm_container.clone();
        let persona_for_rules = self.extract_persona();
        self.client
            .set_game_rules_callback(Arc::new(move |game_rules| {
                info!(
                    "Agent '{}' received game rules update: version {}",
                    agent_name_for_callback, game_rules.version
                );
                // 重新注入 rule_validator（available_actions 可能已变更）
                if let Some(ref handler) = immediate_handler_for_rules {
                    let rule_validator =
                        super::Agent::build_rule_validator(&game_rules.available_actions);
                    let h = handler.clone();
                    let h2 = handler.clone();

                    // 更新决策规则 + 重建 CognitiveImmediateDecisionMaker
                    let rules_update = game_rules
                        .immediate_events
                        .as_ref()
                        .and_then(|e| e.decision_rules.clone());
                    let llm_c = llm_container_for_rules.clone();
                    let persona = persona_for_rules.clone();
                    let agent_name = agent_name_for_callback.clone();

                    tokio::spawn(async move {
                        h.set_rule_validator(rule_validator).await;

                        // 更新决策规则（数据驱动）
                        if let Some(ref rules) = rules_update {
                            h.update_rules(rules.clone()).await;
                        }

                        // 重建 CognitiveImmediateDecisionMaker（复用 LLM + persona）
                        if let Some(ref llm_container) = llm_c {
                            let rules = rules_update.unwrap_or_default();
                            let new_maker = Arc::new(
                                crate::component::immediate::CognitiveImmediateDecisionMaker::new(
                                    llm_container.clone(),
                                    persona,
                                    agent_name,
                                    rules,
                                ),
                            )
                                as Arc<dyn crate::component::immediate::ImmediateDecisionMaker>;
                            let new_handler = h2.with_updated_decision_maker(new_maker);
                            info!("game_rules_callback: 即时事件处理器已热更新");
                            // Handler 的 Arc 字段通过 clone 共享，内部状态已正确更新
                            let _ = new_handler;
                        }
                    });
                }
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

        // 更新即时事件处理器配置（如果有 immediate_events 配置）
        if let Some(ref immediate_events) = game_rules.immediate_events
            && let Some(ref handler) = self.immediate_handler
        {
            // 更新决策规则（数据驱动）
            if let Some(ref rules) = immediate_events.decision_rules {
                handler.update_rules(rules.clone()).await;
            }

            // 重建 CognitiveImmediateDecisionMaker（复用 Agent 持有的 LLM + persona）
            if let Some(ref llm_container) = self.actor_llm_container {
                let rules = immediate_events.decision_rules.clone().unwrap_or_default();
                let persona = self.extract_persona();
                let agent_name = self.character_name().to_string();
                let new_maker = Arc::new(
                    crate::component::immediate::CognitiveImmediateDecisionMaker::new(
                        llm_container.clone(),
                        persona,
                        agent_name,
                        rules,
                    ),
                )
                    as Arc<dyn crate::component::immediate::ImmediateDecisionMaker>;
                let new_handler = handler.with_updated_decision_maker(new_maker);
                self.immediate_handler = Some(Arc::new(new_handler));
                info!("即时事件处理器配置已更新（CognitiveImmediateDecisionMaker）");
            }
        }

        // 绑定即时意图通道到 WebSocket 的统一 intent_tx
        if let Some(ref handler) = self.immediate_handler {
            if let Some(tx) = self.client.intent_sender().await {
                handler.replace_intent_channel(tx).await;
            } else {
                warn!("WebSocket intent_tx 不可用，即时回应将使用临时 channel");
            }
        }

        // 注入规则验证回调到即时事件处理器（Layer 1: action_type 合法性）
        self.inject_rule_validator(&game_rules.available_actions)
            .await;

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

        // 暂存上轮提交的 intents，供天魂生成上一轮叙事用
        let last_intents_for_narrative =
            Arc::new(std::sync::Mutex::new(Vec::<crate::models::Intent>::new()));

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

                    // 更新即时事件处理器 tick_id + 尝试绑定通道
                    if let Some(ref handler) = self.immediate_handler {
                        handler.set_tick_id(world_state.tick_id).await;
                        // 每个 tick 尝试绑定即时意图通道（幂等，首次成功后不再重复）
                        if let Some(tx) = self.client.intent_sender().await {
                            handler.replace_intent_channel(tx).await;
                        }
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

                    // 2.5 社交事件 → 自动更新关系（非阻塞，spawn 后台任务）
                    self.process_social_events(&world_state.events_log, &world_state.entities);

                    // 3. 每 FORGETTING_INTERVAL_TICKS tick 运行遗忘机制
                    if world_state.tick_id % super::FORGETTING_INTERVAL_TICKS == 0
                        && let Err(e) = self.run_forgetting(world_state.tick_id).await {
                            warn!("Failed to run forgetting mechanism: {}", e);
                        }

                    // 4. 构建增强的世界状态（包含记忆上下文 + deferred 对话）
                    let mut memory_context = self.get_memory_context().await;

                    // 4.1 生存压力注入：hunger/thirst 低于阈值时强制注入紧急信号
                    // （在物品注入之后补充具体食物/水名称）
                    let survival_warnings = {
                        let survival_threshold = self.config.survival_threshold();
                        let attrs = &world_state.self_state.attributes;
                        let hunger = attrs.get("hunger").copied().unwrap_or(100);
                        let thirst = attrs.get("thirst").copied().unwrap_or(100);
                        let mut warnings = Vec::new();

                        if hunger > 0 && hunger <= survival_threshold {
                            // 查找背包中的食物
                            let foods: Vec<String> = world_state.self_state.inventory.iter()
                                .filter(|i| i.item_type == cyber_jianghu_protocol::ITEM_TYPE_CONSUMABLE)
                                .map(|i| i.name.clone())
                                .collect();
                            if !foods.is_empty() {
                                warnings.push(format!(
                                    "【生存警告】你正处于极度饥饿状态，必须立即进食！背包中有：{}。使用 eat 命令吃掉其中一个。",
                                    foods.join("、")
                                ));
                            } else {
                                // 查找地上的食物
                                let ground_foods: Vec<String> = world_state.nearby_items.iter()
                                    .filter(|i| i.item_type == cyber_jianghu_protocol::ITEM_TYPE_CONSUMABLE)
                                    .map(|i| i.name.clone())
                                    .collect();
                                if !ground_foods.is_empty() {
                                    warnings.push(format!(
                                        "【生存警告】你正处于极度饥饿状态，必须立即进食！地上有：{}。先 pickup 再 eat。",
                                        ground_foods.join("、")
                                    ));
                                } else {
                                    warnings.push(
                                        "【生存警告】你正处于极度饥饿状态，必须立即进食！背包和地上都没有食物，移动到有资源的地点。".to_string()
                                    );
                                }
                            }
                        }

                        if thirst > 0 && thirst <= survival_threshold {
                            // 数据驱动：通过 item_type=consumable 识别饮品，背包优先
                            let backpack_drinks: Vec<String> = world_state.self_state.inventory.iter()
                                .filter(|i| i.item_type == cyber_jianghu_protocol::ITEM_TYPE_CONSUMABLE)
                                .map(|i| i.name.clone())
                                .collect();
                            let ground_drinks: Vec<String> = world_state.nearby_items.iter()
                                .filter(|i| i.item_type == cyber_jianghu_protocol::ITEM_TYPE_CONSUMABLE)
                                .map(|i| i.name.clone())
                                .collect();

                            if !backpack_drinks.is_empty() {
                                warnings.push(format!(
                                    "【生存警告】你正处于极度口渴状态，必须立即饮水！背包中有：{}。使用 drink 命令饮用。",
                                    backpack_drinks.join("、")
                                ));
                            } else if !ground_drinks.is_empty() {
                                warnings.push(format!(
                                    "【生存警告】你正处于极度口渴状态，必须立即饮水！地上有：{}。先 pickup 再 drink。",
                                    ground_drinks.join("、")
                                ));
                            } else {
                                warnings.push(
                                    "【生存警告】你正处于极度口渴状态，必须立即饮水！附近没有水源，移动到有水的地点。".to_string()
                                );
                            }
                        }
                        warnings
                    };

                    // 注入延迟处理的即时对话（DeferToMainTick 事件）
                    if let Some(ref handler) = self.immediate_handler {
                        let deferred = handler.get_deferred_events().await;
                        if !deferred.is_empty() {
                            let deferred_ctx: Vec<String> = deferred.iter()
                                .filter_map(|e| {
                                    let content = e.metadata.get("content")
                                        .and_then(|v| v.as_str()).unwrap_or("");
                                    if content.is_empty() { None }
                                    else {
                                        let sender = e.metadata.get("from_agent_name")
                                            .and_then(|v| v.as_str()).unwrap_or("有人");
                                        Some(format!("[{}对你说: {}]", sender, content))
                                    }
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

                    // 4.5 天魂叙事生成（将 WorldState 转化为 NarrativeContext 注入人魂）
                    if let Some(ref generator) = self.narrative_generator {
                        let recent = self.memory_manager.as_ref()
                            .map(|m| {
                                m.working().get_top_n(5)
                                    .into_iter()
                                    .map(|e| e.content.clone())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();

                        // 读取上轮提交的 intents
                        let last_intents = last_intents_for_narrative.lock().unwrap().clone();

                        // 数据驱动的上轮行动摘要：从 soul_cycle_recorder 读取上轮人魂叙事
                        // 人魂叙事是中文自然语言（如"拾起桌上两个馒头"），精确且无 ID 泄漏
                        let last_action_summary = if !last_intents.is_empty() {
                            if let Some(recorder) = self.soul_recorder().await {
                                recorder.get_last_renhun_narrative(world_state.tick_id).await
                                    .map(|narrative| format!(
                                        "【重要】你上一轮的行动：{}。不要进行无谓的重复。",
                                        narrative
                                    ))
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        // 天魂生成上一轮叙事（用于 soul_cycle_record 回填）
                        // first_tick: last_execution_summary 为 None 表示首轮（无历史数据）
                        let first_tick = world_state.last_execution_summary.is_none();
                        let execution_narrative = if let Some(ref validator) = self.validator {
                            match validator
                                .generate_execution_narrative(
                                    &last_intents,
                                    world_state.last_execution_summary.as_ref().unwrap_or(&ExecutionSummary {
                                        total: 0,
                                        succeeded: 0,
                                        partial: 0,
                                        failed: 0,
                                        skipped: 0,
                                    }),
                                    first_tick,
                                )
                                .await
                            {
                                Ok(n) => n,
                                Err(e) => {
                                    warn!("天魂生成执行叙事错误: {}", e);
                                    None
                                }
                            }
                        } else {
                            None
                        };

                        // 将 execution_narrative 持久化到上一轮的 soul_cycle_record
                        // world_state.tick_id 是当前tick，narrative 是关于上一轮的执行
                        if let Some(ref narrative) = execution_narrative
                            && world_state.tick_id > 1
                            && let Some(recorder) = self.soul_recorder().await
                        {
                            // Agent 推理频率低于 tick 推进频率，tick_id 不连续
                            // 需要找到实际上一轮有记录的 tick，而非 tick_id - 1
                            if let Some(prev_tick) = recorder.get_last_recorded_tick(world_state.tick_id).await {
                                recorder.update_previous_round_narrative(prev_tick, narrative).await;
                            }
                        }

                        let last_summary = world_state.last_execution_summary.as_ref();
                        match generator.generate(&world_state, last_summary, &recent, execution_narrative.clone()).await {
                            Ok(narrative_ctx) => {
                                // 将 NarrativeContext 的核心内容注入 memory_context
                                let narrative_section = format!(
                                    "\n### 当前感知\n{}\n{}\n{}\n{}",
                                    narrative_ctx.self_perception.status_summary,
                                    narrative_ctx.environment.location_description,
                                    narrative_ctx.self_perception.inventory_narrative,
                                    narrative_ctx.environment.ambient_features,
                                );
                                memory_context.push_str(&narrative_section);

                                // 附近的人
                                if !narrative_ctx.nearby_agents.is_empty() {
                                    let agents: Vec<String> = narrative_ctx.nearby_agents.iter()
                                        .take(5)
                                        .map(|a| format!(
                                            "- {} ({})",
                                            a.appearance, a.current_activity
                                        ))
                                        .collect();
                                    memory_context.push_str(&format!(
                                        "\n附近的人:\n{}", agents.join("\n")
                                    ));
                                }

                                // 结构化物品可用性注入（按来源 + 类型分组，强信号）
                                // 替代旧的 interactive_elements（信息重叠且非结构化）
                                {
                                    let mut sections: Vec<String> = Vec::new();

                                    // 辅助：格式化物品列表
                                    let format_items = |items: &[(&str, &str, i32)]| -> String {
                                        items.iter()
                                            .map(|(name, itype, qty)| {
                                                let type_tag = match *itype {
                                                    t if t == cyber_jianghu_protocol::ITEM_TYPE_CONSUMABLE => "(食物/水)",
                                                    t if t == cyber_jianghu_protocol::ITEM_TYPE_WEAPON => "(武器)",
                                                    t if t == cyber_jianghu_protocol::ITEM_TYPE_MATERIAL => "(材料)",
                                                    t if t == cyber_jianghu_protocol::ITEM_TYPE_CURRENCY => "(货币)",
                                                    _ => "",
                                                };
                                                if *qty > 1 {
                                                    format!("{}x{} {}", name, qty, type_tag)
                                                } else {
                                                    format!("{} {}", name, type_tag)
                                                }
                                            })
                                            .collect::<Vec<_>>()
                                            .join("、")
                                    };

                                    // 背包物品
                                    if !world_state.self_state.inventory.is_empty() {
                                        let items: Vec<(&str, &str, i32)> = world_state.self_state.inventory.iter()
                                            .map(|i| (i.name.as_str(), i.item_type.as_str(), i.quantity))
                                            .collect();
                                        sections.push(format!("**背包**: {}", format_items(&items)));
                                    }

                                    // 地上物品
                                    if !world_state.nearby_items.is_empty() {
                                        let items: Vec<(&str, &str, i32)> = world_state.nearby_items.iter()
                                            .map(|i| (i.name.as_str(), i.item_type.as_str(), i.quantity))
                                            .collect();
                                        sections.push(format!("**地上**: {}", format_items(&items)));
                                    }

                                    // 可采集资源
                                    if !world_state.location.gatherable_items.is_empty() {
                                        let items: Vec<(&str, &str, i32)> = world_state.location.gatherable_items.iter()
                                            .map(|i| (i.name.as_str(), i.item_type.as_str(), 1))
                                            .collect();
                                        sections.push(format!("**可采集**: {}", format_items(&items)));
                                    }

                                    if !sections.is_empty() {
                                        memory_context.push_str(&format!(
                                            "\n\n### 可用资源\n{}\n> 你只能使用以上实际存在的物品。禁止编造不存在的物品。",
                                            sections.join("\n")
                                        ));
                                    }
                                }

                                // 上一轮行动结果（数据驱动优先，避免 LLM 幻觉）
                                if let Some(ref summary) = last_action_summary {
                                    memory_context.push_str(&format!(
                                        "\n### 上一轮行动结果\n{}\n",
                                        summary
                                    ));
                                } else if let Some(ref exec_narr) = execution_narrative {
                                    memory_context.push_str(&format!(
                                        "\n### 上一轮行动结果\n{}\n",
                                        exec_narr
                                    ));
                                } else if let Some(ref outcome) = narrative_ctx.last_outcome {
                                    memory_context.push_str(&format!(
                                        "\n### 上一轮行动结果\n{}\n",
                                        outcome.result_narrative
                                    ));
                                    if !outcome.side_effects.is_empty() {
                                        memory_context.push_str(&format!(
                                            "附带效果: {}\n",
                                            outcome.side_effects.join("；")
                                        ));
                                    }
                                    if !outcome.unexpected_events.is_empty() {
                                        memory_context.push_str(&format!(
                                            "意外事件: {}\n",
                                            outcome.unexpected_events.join("；")
                                        ));
                                    }
                                }

                                debug!("NarrativeContext 注入成功");
                            }
                            Err(e) => {
                                warn!("天魂叙事生成失败，使用原始 memory_context: {}", e);
                            }
                        }
                    }

                    // 4.2 生存压力注入（延迟到物品信息之后，可引用具体物品名）
                    if !survival_warnings.is_empty() {
                        memory_context.push_str("\n### 紧急\n");
                        memory_context.push_str(&survival_warnings.join("\n"));
                    }

                    // 5. 三魂循环：人魂决策 → 天魂审核 → 驳回则重试
                    // 循环直到审查通过或达到最大重试次数
                    let max_retries = self.config.game_rules
                        .as_ref()
                        .and_then(|g| g.intent_batch.as_ref())
                        .map(|b| b.max_retries)
                        .unwrap_or(3);
                    let _max_intents = self.config.game_rules
                        .as_ref()
                        .and_then(|g| g.intent_batch.as_ref())
                        .map(|b| b.max_intents_per_tick)
                        .unwrap_or(5);
                    let agent_id = world_state.agent_id.unwrap_or_default();
                    let mut final_intent = None;

                    for attempt in 0..=max_retries {

                        // 5a. 人魂 (ActorSoul) 决策 — 直连 WorldState，输出结构化 Intent
                        // 优先使用 decision_with_chain_callback（人魂直连 WorldState）
                        let (raw_intent, _cognitive_chain) = {
                            let tick_id = world_state.tick_id;
                            let agent_id = world_state.agent_id.unwrap_or_default();
                            let decision_future = async {
                                // 最高优先级：decision_with_chain_callback（人魂直连 WorldState）
                                if let Some(ref chain_callback) = self.decision_with_chain_callback {
                                    let fb = self.last_rejection_reason.as_deref();
                                    return chain_callback(&world_state, &memory_context, fb).await;
                                }

                                // 降级路径：旧式回调（不接收 WorldState）
                                if let Some(ref reason) = self.last_rejection_reason {
                                    if let Some(ref callback) = self.decision_with_feedback_callback {
                                        let intent = callback(tick_id, agent_id, &memory_context, Some(reason.as_str())).await;
                                        (intent, None)
                                    } else if let Some(ref memory_callback) = self.decision_with_memory_callback {
                                        let combined = if memory_context.is_empty() {
                                            format!("[意图被驳回: {}，请重新决策]", reason)
                                        } else {
                                            format!("{}\n[意图被驳回: {}，请重新决策]", memory_context, reason)
                                        };
                                        let intent = memory_callback(tick_id, agent_id, &combined).await;
                                        (intent, None)
                                    } else {
                                        let intent = (self.decision_callback)(tick_id, agent_id).await;
                                        (intent, None)
                                    }
                                } else if let Some(ref memory_callback) = self.decision_with_memory_callback {
                                    let intent = memory_callback(tick_id, agent_id, &memory_context).await;
                                    (intent, None)
                                } else {
                                    let intent = (self.decision_callback)(tick_id, agent_id).await;
                                    (intent, None)
                                }
                            };

                            decision_future.await
                        };

                        // 如果 final_intent 已被设置（如 speak 即时通道），退出
                        if final_intent.is_some() { break; }

                        // 记录人魂输出（结构化 Intent）
                        let renhun_action = raw_intent.action_type.as_str();
                        let renhun_action_data = raw_intent.action_data
                            .as_ref()
                            .map(|d| serde_json::to_string(d).unwrap_or_default())
                            .unwrap_or_default();
                        let renhun_thought_log = raw_intent.thought_log.as_deref().unwrap_or("");
                        if let Some(recorder) = self.soul_recorder().await {
                            recorder.record_renhun(
                                world_state.tick_id,
                                attempt,
                                &format!("{} {}", renhun_action, renhun_action_data),
                                renhun_thought_log,
                            ).await;
                            // 记录游戏内时间和现实时间
                            let world_time_str = Self::format_world_time(&world_state.world_time);
                            recorder.record_world_time(world_state.tick_id, attempt, &world_time_str).await;
                        }

                        // 5b. 地魂翻译步骤已消除 — 人魂直接输出结构化 Intent
                        // 记录地魂为空（兼容 soul_cycle_recorder）
                        if let Some(recorder) = self.soul_recorder().await {
                            recorder.record_tianhun(
                                world_state.tick_id,
                                attempt,
                                None, // 地魂已消除
                                None,
                                None,
                                false,
                                Some("人魂直连 WorldState，地魂翻译已消除"),
                            ).await;
                        }

                        // 5b'. speak 即时通道检测
                        // 人魂直连后，speak intent 直接从 raw_intent 提取（不再依赖地魂拆分）
                        let multi_translation = crate::soul::translator::MultiTranslationResult {
                            intents: vec![raw_intent.clone()],
                            speech_intent: None,
                            original_narrative: String::new(),
                            original_thought_log: raw_intent.thought_log.as_deref().unwrap_or("").to_string(),
                        };

                        // 5c. 天魂 (ReflectorSoul) 审核 — 直接审查人魂输出的结构化 Intent
                        // 分级审核策略：根据 action_type 决定审核级别（Always/Adaptive/Skip）
                        let graded_config = self.config.game_rules
                            .as_ref()
                            .and_then(|g| g.intent_batch.as_ref())
                            .map(|b| b.llm_validation.clone());

                        let mut approved_intents = Vec::new();
                        let mut batch_rejection: Option<String> = None;
                        let mut batch_layers: Vec<super::agent::LayerResult> = Vec::new();
                        let mut batch_narrative: Option<String> = None;

                        for intent in multi_translation.intents {
                            // 分级决策：Skip 类型只做 RuleEngine（跳过 LLM）
                            let skip_llm = Self::should_skip_llm_validation(
                                &intent, graded_config.as_ref(),
                            );

                            if skip_llm {
                                // 仅做 Layer 1 (action_type) + Layer 2 (RuleEngine)
                                match self.validate_rules_only(&intent, &world_state).await {
                                    Ok(()) => {
                                        // 记录通过的两个 layer
                                        if batch_layers.is_empty() {
                                            batch_layers.push(super::agent::LayerResult {
                                                layer: "layer1",
                                                passed: true,
                                                detail: None,
                                            });
                                            batch_layers.push(super::agent::LayerResult {
                                                layer: "layer2",
                                                passed: true,
                                                detail: None,
                                            });
                                        }
                                        approved_intents.push(intent);
                                    }
                                    Err(reason) => {
                                        warn!("Tick {} 分级审核（Skip）驳回: {}", world_state.tick_id, reason);
                                        batch_rejection = Some(reason.clone());
                                        batch_layers.push(super::agent::LayerResult {
                                            layer: "layer1",
                                            passed: true,
                                            detail: None,
                                        });
                                        batch_layers.push(super::agent::LayerResult {
                                            layer: "layer2",
                                            passed: false,
                                            detail: Some(reason),
                                        });
                                    }
                                }
                            } else {
                                // 完整三层审查（含 LLM）
                                match self.validate_with_reflector(intent, &world_state).await? {
                                    super::agent::ReflectorResult::Approved { intent: approved, layers, narrative } => {
                                        batch_layers = layers;
                                        batch_narrative = narrative;
                                        approved_intents.push(approved);
                                    }
                                    super::agent::ReflectorResult::Rejected { reason, layers } => {
                                        batch_layers = layers;
                                        batch_rejection = Some(reason.clone());
                                        // 叙事化驳回原因
                                        let narrated = super::Agent::narrativize_rejection(&reason);
                                        self.last_rejection_reason = Some(narrated.clone());
                                        warn!("Tick {} 第 {} 次天魂审查驳回: {}", world_state.tick_id, attempt, reason);
                                    }
                                }
                            }

                            // primary intent 被驳回则终止批次（Pipeline 语义）
                            if batch_rejection.is_some() {
                                break;
                            }
                        }

                        if !approved_intents.is_empty() {
                            // 记录天魂审查结果
                            if let Some(recorder) = self.soul_recorder().await {
                                let layer1 = batch_layers.iter().find(|l| l.layer == "layer1");
                                let layer2 = batch_layers.iter().find(|l| l.layer == "layer2");
                                let layer3 = batch_layers.iter().find(|l| l.layer == "layer3");
                                recorder.record_dihun(
                                    world_state.tick_id,
                                    attempt,
                                    "approved",
                                    layer1.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                    layer2.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                    layer3.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                    None,
                                    batch_narrative.as_deref(),
                                ).await;
                                let pipeline = Self::assemble_pipeline(approved_intents.clone());
                                recorder.record_final_intent(
                                    world_state.tick_id,
                                    attempt,
                                    Some(&pipeline.intent_id.to_string()),
                                    Some(pipeline.action_type.as_str()),
                                    pipeline.action_data.as_ref().map(|d| serde_json::to_string(d).unwrap_or_default()).as_deref(),
                                ).await;
                                final_intent = Some(pipeline);
                            } else {
                                let pipeline = Self::assemble_pipeline(approved_intents.clone());
                                final_intent = Some(pipeline);
                            }
                            // 暂存 approved intents，供下一轮天魂生成叙事用
                            if let Ok(mut saved) = last_intents_for_narrative.lock() {
                                saved.clone_from(&approved_intents);
                            } else {
                                warn!("暂存 approved_intents 失败：Mutex lock 获取失败");
                            }
                            break;
                        } else if let Some(reason) = batch_rejection {
                            // 记录驳回
                            if let Some(recorder) = self.soul_recorder().await {
                                let layer1 = batch_layers.iter().find(|l| l.layer == "layer1");
                                let layer2 = batch_layers.iter().find(|l| l.layer == "layer2");
                                let layer3 = batch_layers.iter().find(|l| l.layer == "layer3");
                                let narrated = super::Agent::narrativize_rejection(&reason);
                                recorder.record_dihun(
                                    world_state.tick_id,
                                    attempt,
                                    "rejected",
                                    layer1.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                    layer2.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                    layer3.map(|l| l.detail.as_deref().unwrap_or("通过")),
                                    Some(&reason),
                                    Some(&narrated),
                                ).await;
                            }

                            if attempt >= max_retries {
                                warn!("Tick {} 达到最大重试次数 {}，提交 idle", world_state.tick_id, max_retries);
                                final_intent = Some(Intent::new(agent_id, world_state.tick_id, "idle", None)
                                    .with_thought(format!("意图多次被驳回: {}", reason)));
                                break;
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
                            // 发送最后一个 idle 意图后退出（通过天魂规则验证保持不变量）
                            let agent_id = self.client.agent_id().await.unwrap_or_default();
                            let death_idle = Intent::new(
                                agent_id,
                                world_state.tick_id,
                                "idle",
                                None,
                            );
                            if self.validate_rules_only(&death_idle, &world_state).await.is_ok() {
                                self.client.send_intent(&death_idle).await.ok();
                            }
                            return Ok(());
                        }
                    }

                    // 7. 天魂验证 + 发送意图
                    // 天魂唯一出入口：ALL intents 离开 Agent 前必须经过天魂验证
                    // 正常三魂循环产出的 intent 已通过 5c 审查，此处验证 idle fallback 路径
                    if let Err(reason) = self.validate_rules_only(&final_intent, &world_state).await {
                        warn!("Tick {} 最终 intent 被天魂规则验证驳回: {}，跳过发送", world_state.tick_id, reason);
                        if let Some(ref handler) = self.immediate_handler {
                            handler.set_current_intent(None).await;
                        }
                    } else {
                        // 设置当前意图类型，让即时事件处理器进行冲突检测
                        if let Some(ref handler) = self.immediate_handler {
                            handler.set_current_intent(Some(final_intent.action_type.to_string())).await;
                        }

                        if let Err(e) = self.client.send_intent(&final_intent).await {
                            error!("Failed to send intent: {}", e);
                            if let Some(ref handler) = self.immediate_handler {
                                handler.set_current_intent(None).await;
                            }
                            if let Err(reconnect_err) = self.reconnect().await {
                                error!("Reconnect failed: {}", reconnect_err);
                            }
                        } else {
                            info!(
                                "Intent sent successfully: tick={}, action={}, agent={}",
                                final_intent.tick_id, final_intent.action_type, final_intent.agent_id
                            );

                            // 实时模式：poll ExecutionResult（server 立即处理后的反馈）
                            // 短暂等待 200ms 后检查，避免 busy-wait
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                            match self.client.try_receive_execution_result().await {
                                Ok(Some(result)) => {
                                    if result.success {
                                        debug!(
                                            "ExecutionResult: tick={}, intent={}, success",
                                            result.tick_id, result.intent_id
                                        );
                                    } else {
                                        warn!(
                                            "ExecutionResult: tick={}, intent={}, FAILED: {}",
                                            result.tick_id,
                                            result.intent_id,
                                            result.error.as_deref().unwrap_or("unknown")
                                        );
                                        // 注入失败原因到下轮推理上下文
                                        let reason = result.error.unwrap_or_default();
                                        self.last_rejection_reason = Some(
                                            format!("[意图执行失败: {}]", reason)
                                        );
                                    }
                                }
                                Ok(None) => {
                                    debug!("No ExecutionResult yet (server may be batching)");
                                }
                                Err(e) => {
                                    debug!("ExecutionResult poll error: {}", e);
                                }
                            }

                            if final_intent.action_type.as_str() != "idle" {
                                self.consecutive_idle_count = 0;
                                if let Some(ref container) = self.actor_llm_container {
                                    let llm = container.read().await;
                                    llm.reset_idle_count();
                                }
                            }
                            if final_intent.action_type.as_str() == "idle" {
                                self.maybe_rotate_model().await;
                            }
                            if let Some(ref handler) = self.immediate_handler {
                                handler.set_current_intent(None).await;
                            }

                            // 7.5 上报三魂循环元数据到服务器（使 server-web 可见）
                            let tick_id_for_report = final_intent.tick_id;
                            if let Some(recorder) = self.soul_recorder().await {
                                let records = recorder.get_by_tick(tick_id_for_report).await;
                                let immediate_records = recorder.get_immediate_by_tick(tick_id_for_report).await;

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
                                            narrative: r.previous_round_narrative,
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

                                let mut reported = false;
                                for attempt in 0..3 {
                                    match self.client.send_soul_cycle_report(tick_id_for_report, metadata.clone()).await {
                                        Ok(()) => {
                                            debug!("三魂循环元数据上报成功: tick={}", tick_id_for_report);
                                            reported = true;
                                            break;
                                        }
                                        Err(e) => {
                                            warn!("三魂循环元数据上报失败 (尝试 {}/3): tick={}, err={}", attempt + 1, tick_id_for_report, e);
                                            if attempt < 2 {
                                                tokio::time::sleep(tokio::time::Duration::from_millis(100 * (1 << attempt))).await;
                                            }
                                        }
                                    }
                                }
                                if !reported {
                                    error!("三魂循环元数据上报最终失败: tick={}", tick_id_for_report);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// 发送即时 Intent（统一走主 intent 通道）
    ///
    /// 天魂（旧地魂）路由出的 speak/whisper 或混合说话走此通道，
    /// 与 ImmediateEventHandler 的 RespondNow 共享同一条 WebSocket channel。
    #[allow(dead_code)]
    async fn send_immediate_intent(&self, intent: &Intent) -> std::result::Result<(), String> {
        // 即时意图 per-tick rate limit
        if let Some(handler) = &self.immediate_handler
            && !handler.check_and_increment_send_count(intent.tick_id).await
        {
            return Err("本 tick 即时意图已达上限".to_string());
        }

        if let Err(e) = self.client.send_intent(intent).await {
            warn!(
                "[天魂/即时] intent 发送失败 ({}): {}",
                intent.action_type, e
            );
            Err(e.to_string())
        } else {
            info!(
                "[天魂/即时] intent 已发送: {} {:?}",
                intent.action_type, intent.action_data
            );
            Ok(())
        }
    }
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
