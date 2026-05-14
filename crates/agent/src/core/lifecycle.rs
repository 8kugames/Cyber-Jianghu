// ============================================================================
// 连接生命周期管理
// ============================================================================
//
// 处理 Agent 的连接、主循环和关闭
// 重连逻辑在 reconnect.rs 中
// ============================================================================

use anyhow::Result;
use cyber_jianghu_protocol::{CalendarConfig, ServerMessage, WorldTime};
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::reconnect::{save_character_config_to_fs, should_log_retry};
use crate::component::memory::backend::MemoryBackend;
use crate::component::social::RelationshipStore;
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
            tokio::time::sleep(tokio::time::Duration::from_secs(
                self.config.llm.reconnect_delay_secs,
            ))
            .await;
        }
        info!("Agent '{}' connected to server", self.character_name());

        // 设置游戏规则更新回调
        let agent_name_for_callback = self.character_name().to_string();
        let agent_name_for_skills = agent_name_for_callback.clone();
        let agent_name_for_prompt = agent_name_for_callback.clone();
        self.client
            .set_game_rules_callback(Arc::new(move |game_rules| {
                info!(
                    "Agent '{}' received game rules update: version {}",
                    agent_name_for_callback, game_rules.version
                );
            }))
            .await;

        // 设置技能配置更新回调
        let cognitive_engine_for_skills = self.cognitive_engine.clone();
        self.client
            .set_skill_update_callback(Arc::new(move |skills, removed_items| {
                info!(
                    "Agent '{}' received skill config update: {} skills, {} removed",
                    agent_name_for_skills,
                    skills.len(),
                    removed_items.len()
                );
                if let Some(ref engine) = cognitive_engine_for_skills {
                    engine.update_skill_cache(skills, removed_items);
                }
            }))
            .await;

        // 设置 Prompt 模板配置更新回调
        let cognitive_engine_for_prompt = self.cognitive_engine.clone();
        let validator_for_prompt = self.validator.clone();
        self.client
            .set_prompt_template_callback(Arc::new(
                move |config: cyber_jianghu_protocol::PromptTemplateConfig| {
                    info!(
                        "Agent '{}' received prompt_templates config update: version={}",
                        agent_name_for_prompt, config.version
                    );
                    // 更新人魂 CognitiveEngine
                    if let Some(ref engine) = cognitive_engine_for_prompt {
                        engine.update_prompt_template_from_config(config.clone());
                        engine.save_prompt_template_to_disk();
                    }
                    // 更新天魂 RuleEngine reject 反馈模板
                    if let Some(ref validator) = validator_for_prompt {
                        validator.update_prompt_config(std::sync::Arc::new(config));
                    }
                },
            ))
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

        // 从文件加载角色配置（优先于内存中的旧配置，确保 rebirth 后更新）
        if !agent_id.is_nil() {
            let server_dir = self.config.server_dir(&self.config.server.ws_url);
            let characters_dir = server_dir.join("characters");
            let char_dir = characters_dir.join(agent_id.to_string());
            let char_yaml = char_dir.join("character.yaml");

            if char_yaml.exists() {
                if let Ok(loaded) = crate::config::CharacterConfig::from_file(&char_yaml) {
                    self.character_config = Some(loaded);
                    info!("已从文件加载角色配置: {}", char_yaml.display());
                }
            } else if self.character_config.is_none()
                || self.character_config.as_ref().and_then(|c| c.agent_id) != Some(agent_id)
            {
                // 文件不存在且内存中无匹配配置 → 自动重建
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
                    metadata: None,
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
                    rebirth_delay_ticks: self.config.rebirth_delay_ticks(),
                    metadata: None,
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
        if let Some(ref api_state) = self.http_api_state {
            *api_state.game_rules.write().await = Some(game_rules.clone());
        }

        // 热更新认知引擎的动作别名映射（翻译层依赖 AvailableAction）
        if let Some(ref engine) = self.cognitive_engine {
            engine.update_action_aliases(&game_rules.available_actions);
        }

        // 初始化对话上下文管理器（Fail-Fast: dialogue_context 段存在时所有字段必填）
        if self.dialogue_manager.is_none() {
            #[allow(clippy::collapsible_if)]
            if let Some(ref config) = game_rules.dialogue_context {
                self.init_dialogue_manager(
                    config.max_sessions,
                    config.max_rounds_per_session,
                    config.session_timeout_ticks,
                    config.dialogue_action_types.clone(),
                );
            }
        }

        // 启动时主动拉取 prompt_templates 并写盘
        self.fetch_prompt_templates_from_server().await;

        // 即时事件处理器：新架构下无需热更新（EventStore 配置在 open 时绑定）
        // tick_id 在主循环每个 tick 更新

        // 设置 Server 消息回调（链式：lifecycle 处理 + binary 回调透传）
        // 保留 binary 设置的回调（AgentDied 处理 + 外部 LLM downstream 转发）
        let prev_callback = self.client.get_server_msg_callback().await;
        let immediate_handler = self.immediate_handler.clone();
        let error_feedback = self.server_error_feedback.clone();
        let _event_buffer = self.immediate_event_buffer.clone(); // 保留用于Future ImmediateEvent扩展
        let memory_manager = self.memory_manager.clone();
        let dialogue_manager = self.dialogue_manager.clone();
        let game_rules = self.config.game_rules.clone();
        let current_tick = self.current_tick.clone();
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
                // 2. ImmediateEvent: DB 写入 + Notify（新架构，无 LLM 调用）
                if let ServerMessage::ImmediateEvent { .. } = &msg
                    && let Some(ref handler) = immediate_handler
                {
                    let h = handler.clone();
                    let msg = msg.clone();
                    tokio::spawn(async move {
                        h.handle_server_message(msg).await;
                    });
                }
                // 2b. Dialogue: 写入 DialogueContextManager（统一 game tick 时间域）
                if let ServerMessage::Dialogue { message } = &msg {
                    use cyber_jianghu_protocol::DialogueMessage;
                    use crate::component::dialogue::DialogueRole;

                    let dm = dialogue_manager.clone();
                    let dialogue_message = message.clone();
                    let tick = current_tick.load(std::sync::atomic::Ordering::Relaxed);

                    tokio::spawn(async move {
                        let Some(ref dm) = dm else { return; };
                        let mut guard = dm.write().await;

                        match dialogue_message {
                            DialogueMessage::Content { session_id, from_agent_id, content } => {
                                guard.add_message(
                                    &session_id,
                                    from_agent_id,
                                    DialogueRole::Partner,
                                    &content,
                                    tick,
                                );
                            }
                            DialogueMessage::Request { from_agent_id, opening_remark, .. } => {
                                let session_id = format!("request_{}_{}", from_agent_id, chrono::Utc::now().timestamp());
                                guard.add_message(
                                    &session_id,
                                    from_agent_id,
                                    DialogueRole::Partner,
                                    &opening_remark,
                                    tick,
                                );
                            }
                            DialogueMessage::Accept { session_id, from_agent_id } => {
                                let pending_id = format!("{}{}", crate::component::dialogue::PENDING_SESSION_PREFIX, from_agent_id);
                                guard.migrate_session(&pending_id, &session_id, from_agent_id, tick);
                                guard.add_message(
                                    &session_id,
                                    from_agent_id,
                                    DialogueRole::Partner,
                                    "[对方接受了对话请求]",
                                    tick,
                                );
                            }
                            DialogueMessage::Reject { session_id, from_agent_id, reason } => {
                                let pending_id = format!("{}{}", crate::component::dialogue::PENDING_SESSION_PREFIX, from_agent_id);
                                guard.close_session(&pending_id);
                                warn!(
                                    "Dialogue rejected by {}: session={}, reason={:?}",
                                    from_agent_id, session_id, reason
                                );
                            }
                            DialogueMessage::End { session_id, .. } => {
                                guard.end_session(&session_id);
                            }
                        }
                    });
                }
                // 2c. DailySummaryData：存储到 episodic memory（服务器侧权威动作统计）
                if let ServerMessage::DailySummaryData {
                    game_day,
                    action_counts,
                    location_history,
                    success_count,
                    failure_count,
                    total_actions,
                } = &msg
                {
                    let mm = memory_manager.clone();
                    let gr = game_rules.clone();
                    // Clone owned data before spawn (tokio::spawn requires 'static)
                    let gd = *game_day;
                    let ac = action_counts.clone();
                    let lh = location_history.clone();
                    let sc = *success_count;
                    let fc = *failure_count;
                    let ta = *total_actions;
                    tokio::spawn(async move {
                        if let Some(ref mgr) = mm {
                            let importance = gr
                                .as_ref()
                                .and_then(|g| g.immediate_events.as_ref())
                                .and_then(|ie| ie.event_triage.as_ref())
                                .map(|et| et.daily_summary_importance as f32)
                                .unwrap_or(0.8);
                            // 格式化动作统计
                            let mut sorted: Vec<_> = ac.iter().collect();
                            sorted.sort_by(|a, b| b.1.cmp(a.1));
                            let action_parts: Vec<String> = sorted
                                .iter()
                                .take(5)
                                .map(|(k, v)| format!("{}x{}", k, v))
                                .collect();
                            let content = format!(
                                "第{}游戏日动作统计：共{}次（成{}、败{}）。动作：{}{}",
                                gd,
                                ta,
                                sc,
                                fc,
                                action_parts.join("、"),
                                if lh.is_empty() {
                                    String::new()
                                } else {
                                    format!("；足迹：{}", lh.join("→"))
                                }
                            );
                            let mut entry = crate::component::memory::MemoryEntry::new(
                                Uuid::nil(),
                                gd,
                                content,
                            )
                            .with_event_type("daily_action_stats".to_string())
                            .with_importance(importance);
                            let mut guard = mgr.write().await;
                            if let Err(e) = guard.episodic_mut().add(&mut entry).await {
                                warn!("游戏日{}动作统计写入 episodic memory 失败: {}", gd, e);
                            }
                        }
                    });
                }
                // 3. 透传给 binary 回调（AgentDied 处理、Claw 模式 OpenClaw 转发等）
                if let Some(ref prev) = prev_callback {
                    prev(msg);
                }
            });
        self.client.set_server_msg_callback(callback).await;
        info!("Server 消息回调已注册（即时事件 + 验证错误 + 链式透传）");

        // 暂存上轮提交的 intents，供天魂生成上一轮执行叙事用
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
                    info!("[main] 收到重连请求: {} (agent_id: {:?})", req.ws_url, req.agent_id);
                    // 推断 HTTP URL
                    let http_url = crate::config::ws_to_http_url(&req.ws_url);
                    // 更新客户端 URL
                    self.client.update_server_url(req.ws_url.clone(), http_url).await;
                    // 设置 agent_id (如果需要切换)
                    if let Some(id) = req.agent_id {
                        self.client.set_agent_id(Some(id)).await;
                    }
                    // 触发重连
                    self.reconnect().await?;
                    continue;
                }

                // 重生完成通知（auto-rebirth 成功后唤醒 tick 循环）
                _ = async {
                    if let Some(ref api_state) = self.http_api_state {
                        api_state.rebirth_notify.notified().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    let is_rebirth_done = self.http_api_state.as_ref()
                        .map(|s| !s.is_dead.load(std::sync::atomic::Ordering::Relaxed))
                        .unwrap_or(false);
                    if is_rebirth_done && self.death_reported {
                        self.death_reported = false;
                        self.death_tick_id = None;

                        // 读取 auto-rebirth 产出的 new_agent_id
                        let new_agent_id = if let Some(ref api_state) = self.http_api_state {
                            api_state.pending_rebirth_agent_id.write().await.take()
                        } else {
                            None
                        };

                        if let Some(new_id) = new_agent_id {
                            // P2 fix: 更新 HttpApiState.agent_id
                            if let Some(ref api_state) = self.http_api_state {
                                *api_state.agent_id.write().await = new_id;
                            }

                            // 更新本地 character_config：复用旧角色信息，仅换 agent_id + 状态
                            if let Some(ref mut char_cfg) = self.character_config {
                                char_cfg.agent_id = Some(new_id);
                                char_cfg.status = crate::config::CharacterStatus::Alive;
                                if let Some(ref api_state) = self.http_api_state {
                                    let dir = api_state.character_dir.read().await.clone();
                                    if let Err(e) = save_character_config_to_fs(char_cfg, &dir) {
                                        warn!("自动重生: 保存角色配置失败: {}", e);
                                    }
                                }
                            }

                            // 转世重生：重新 open RelationshipStore（新 agent_id → 新 DB 文件）
                            if let Some(ref api_state) = self.http_api_state {
                                let new_rel_path = api_state.data_dir.join(format!("relationships_{}.db", new_id));
                                match RelationshipStore::open(new_id, &new_rel_path) {
                                    Ok(new_store) => {
                                        // 更新 Agent 级别引用
                                        self.relationship_store = Some(new_store.clone());
                                        // 同步更新 CognitiveEngine 内部引用
                                        if let Some(ref engine) = self.cognitive_engine {
                                            engine.set_relationship_store(new_store.clone());
                                        }
                                        // 同步更新 HttpApiState 引用
                                        *api_state.relationship_store.write().unwrap() = Some(Arc::new(new_store));
                                        info!("转世重生: RelationshipStore 已重初始化 (new_id={})", new_id);
                                    }
                                    Err(e) => {
                                        warn!("转世重生: RelationshipStore 重初始化失败: {}", e);
                                    }
                                }
                            }

                            info!(
                                "Agent '{}' 自动转世完成: new_agent_id={}",
                                self.character_name(), new_id
                            );
                            // 用 new_agent_id reconnect
                            self.client.set_agent_id(Some(new_id)).await;
                        } else {
                            // fallback: 无 pending agent_id 时走 nil reconnect
                            self.client.set_agent_id(None).await;
                        }

                        self.reconnect().await?;
                    }
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

                    // 更新即时事件处理器 tick_id + Session Triage 生命周期管理
                    self.current_tick.store(world_state.tick_id, std::sync::atomic::Ordering::Relaxed);
                    if let Some(ref dm) = self.dialogue_manager {
                        let mut guard = dm.write().await;
                        guard.cleanup_timed_out(world_state.tick_id);
                    }
                    if let Some(ref handler) = self.immediate_handler {
                        handler.set_tick_id(world_state.tick_id).await;
                        let game_day = Self::compute_game_day(
                            &world_state.world_time,
                            self.config.game_rules.as_ref().and_then(|g| g.calendar.as_ref()),
                        );

                        // Session Triage Engine 生命周期：每游戏日重生
                        let need_spawn = match self.session_triage_handle {
                            None => true, // 首次：无 handle
                            Some(ref handle) => handle.is_finished(),
                        };
                        if need_spawn {
                            // 在更新 current_game_day 之前保存旧值（供旧 engine 摘要使用）
                            let prev_game_day = self.session_triage_game_day.take();
                            // 更新共享 game_day（新 engine 可见；旧 engine 检测到 day 改变而退出）
                            handler.set_game_day(game_day).await;
                            self.session_triage_game_day = Some(game_day);
                            if let Some(old_handle) = self.session_triage_handle.take() {
                                match old_handle.await {
                                    Ok(summary_opt) => {
                                        if let Some(ref summary) = summary_opt {
                                            let summary_game_day = prev_game_day.unwrap_or(game_day);
                                            // 写入 episodic memory
                                            if let Some(ref mm) = self.memory_manager {
                                                let importance = self.config.game_rules
                                                    .as_ref()
                                                    .and_then(|g| g.immediate_events.as_ref())
                                                    .and_then(|ie| ie.event_triage.as_ref())
                                                    .map(|et| et.daily_summary_importance as f32)
                                                    .unwrap_or(0.8);
                                                let mut entry = crate::component::memory::MemoryEntry::new(
                                                    world_state.agent_id.unwrap_or_default(),
                                                    world_state.tick_id,
                                                    summary.clone(),
                                                )
                                                .with_event_type("daily_summary".to_string())
                                                .with_importance(importance);
                                                let mut mm_guard = mm.write().await;
                                                use crate::component::memory::backend::MemoryBackend;
                                                match mm_guard.episodic_mut().add(&mut entry).await {
                                                    Ok(_) => {
                                                        info!(
                                                            "游戏日 {} 摘要已存储到 episodic memory (importance={:.1})",
                                                            summary_game_day, importance
                                                        );
                                                    }
                                                    Err(e) => {
                                                        warn!("游戏日摘要写入 episodic memory 失败: {}", e);
                                                    }
                                                }
                                            }

                                            // 提交每日摘要到 Server（重试 + 指数退避）
                                            let ds_config = self.config.game_rules
                                                .as_ref()
                                                .and_then(|g| g.daily_summary.as_ref());
                                            let max_retries = ds_config.map(|c| c.max_retries).unwrap_or(3);
                                            let base_delay_ms = ds_config.map(|c| (c.ttl_ticks as u64).min(1000)).unwrap_or(100);

                                            let mut submitted = false;
                                            for attempt in 0..max_retries {
                                                match self.client.send_daily_summary(summary_game_day, summary).await {
                                                    Ok(()) => {
                                                        info!(
                                                            "游戏日 {} 摘要已提交 Server (attempt {})",
                                                            summary_game_day, attempt + 1
                                                        );
                                                        submitted = true;
                                                        break;
                                                    }
                                                    Err(e) => {
                                                        warn!(
                                                            "游戏日 {} 摘要提交 Server 失败 (attempt {}/{}): {}",
                                                            summary_game_day, attempt + 1, max_retries, e
                                                        );
                                                        if attempt + 1 < max_retries {
                                                            let delay = base_delay_ms * (1 << attempt);
                                                            tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                                                        }
                                                    }
                                                }
                                            }
                                            if !submitted {
                                                warn!(
                                                    "游戏日 {} 摘要提交 Server 最终失败（已重试 {} 次）",
                                                    summary_game_day, max_retries
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        if e.is_panic() {
                                            warn!("SessionTriageEngine panic（将被重启）: {}", e);
                                        } else {
                                            warn!("SessionTriageEngine 被取消: {}", e);
                                        }
                                    }
                                }
                            }
                            // spawn 新 SessionTriageEngine
                            if let Some(ref llm_container) = self.actor_llm_container {
                                let triage_config = handler.event_store().config().clone();
                                let engine = crate::component::immediate::SessionTriageEngine::new(
                                    handler.event_store().clone(),
                                    llm_container.clone(),
                                    self.extract_persona(),
                                    self.character_name().to_string(),
                                    triage_config,
                                    game_day,
                                    handler.current_game_day(),
                                    Some(world_state.world_time.clone()),
                                );
                                self.session_triage_handle = Some(tokio::spawn(engine.run()));
                                info!(
                                    "SessionTriageEngine 已 spawn: agent={}, game_day={}",
                                    self.character_name(), game_day
                                );
                            }
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
                    // 路径 1: WorldState.events_log 中包含 DeathNotification
                    // 路径 2: AgentDied WS 回调已设置 is_dead=true，但 events_log 可能已过期
                    let death_via_events = !self.death_reported
                        && world_state.events_log.iter().any(|e| {
                            e.event_type == cyber_jianghu_protocol::WorldEventType::DeathNotification
                        });
                    let death_via_callback = !self.death_reported
                        && self.http_api_state.as_ref()
                            .map(|s| s.is_dead.load(std::sync::atomic::Ordering::Relaxed))
                            .unwrap_or(false);

                    if death_via_events || death_via_callback {
                            let death_desc = world_state.events_log.iter()
                                .find(|e| e.event_type == cyber_jianghu_protocol::WorldEventType::DeathNotification)
                                .map(|e| e.description.as_str())
                                .unwrap_or("AgentDied 回调通知（events_log 未包含 DeathNotification）");
                            warn!(
                                "Agent '{}' has died: {}",
                                self.character_name(), death_desc
                            );
                            self.death_reported = true;
                            self.death_tick_id = Some(world_state.tick_id);

                            // 从 HttpApiState 读取 rebirth_delay_ticks（由 AgentDied 回调写入）
                            if let Some(ref api_state) = self.http_api_state {
                                api_state.is_dead.store(true, std::sync::atomic::Ordering::Relaxed);
                                self.rebirth_delay_ticks = api_state.rebirth_delay_ticks.load(std::sync::atomic::Ordering::Relaxed);
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

                            // 死亡时触发传记生成（fire-and-forget，不阻塞 rebirth 调度）
                            if let Some(ref api_state) = self.http_api_state {
                                let state = api_state.clone();
                                let dead_agent_id = world_state.agent_id.unwrap_or_default();
                                tokio::spawn(async move {
                                    info!("[biography] 死亡触发传记生成: agent={}", dead_agent_id);
                                    // 最多重试 3 次，间隔 30s（LLM 瞬断/rate limit 等临时错误可恢复）
                                    const MAX_RETRIES: u32 = 3;
                                    const RETRY_DELAY_SECS: u64 = 30;
                                    for attempt in 0..MAX_RETRIES {
                                        match crate::infra::api::handlers::generate_biography_for_agent(&state, dead_agent_id).await {
                                            Ok(bio) => {
                                                info!("[biography] 死亡传记生成成功: {}字", bio.chars().count());
                                                return;
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "[biography] 死亡传记生成失败 (attempt {}/{}): {}",
                                                    attempt + 1, MAX_RETRIES, e
                                                );
                                                if attempt + 1 < MAX_RETRIES {
                                                    tokio::time::sleep(std::time::Duration::from_secs(RETRY_DELAY_SECS)).await;
                                                }
                                            }
                                        }
                                    }
                                    warn!("[biography] 死亡传记生成最终失败: agent={}", dead_agent_id);
                                });
                            }

                            // 自动重生：调度延迟后的转世重生 API 调用
                            let auto_rebirth_enabled = self.http_api_state
                                .as_ref()
                                .map(|s| s.auto_rebirth.load(std::sync::atomic::Ordering::Relaxed))
                                .unwrap_or(true);
                            if self.rebirth_delay_ticks > 0 && auto_rebirth_enabled {
                                let delay_ticks = self.rebirth_delay_ticks;
                                let tick_secs = self.get_tick_duration().await.as_secs();
                                let delay_ms = delay_ticks as u64 * tick_secs * 1000;
                                // world_state.agent_id 在 agent 死亡后可能为 None
                                // （server 不返回 dead agent 的 agent_id），
                                // 此时从 character_config 获取最后已知的 agent_id，
                                // 避免 nil UUID 导致 auto-rebirth API 永久失败。
                                let old_agent_id = world_state.agent_id
                                    .or_else(|| {
                                        self.character_config.as_ref().and_then(|c| c.agent_id)
                                    })
                                    .unwrap_or_default();
                                let http_url = self.config.server.http_url.clone();
                                let api_state = self.http_api_state.clone();

                                // 提取 device_id + auth_token
                                let Some(device_cfg) = self.device_config.as_ref() else {
                                    warn!("自动转世重生跳过: device_config 未设置");
                                    continue;
                                };
                                let device_id = device_cfg.device_id;
                                let auth_token = device_cfg.auth_token.clone();

                                // 复用旧角色 name + system_prompt（转世：同角色新 agent_id）
                                let (name, system_prompt) = self.character_config
                                    .as_ref()
                                    .map(|cc| (cc.name.clone(), cc.system_prompt.clone().unwrap_or_default()))
                                    .unwrap_or_default();

                                // 重试参数（数据驱动，从 GameRules 配置读取）
                                let retry_max = self.config.game_rules
                                    .as_ref()
                                    .map(|r| r.rebirth_retry_max_attempts)
                                    .unwrap_or(3);
                                let retry_interval = std::time::Duration::from_secs(
                                    self.config.game_rules
                                        .as_ref()
                                        .map(|r| r.rebirth_retry_interval_secs)
                                        .unwrap_or(30)
                                );

                                if old_agent_id == uuid::Uuid::nil() {
                                    warn!(
                                        "自动转世重生跳过: 无法获取有效的 old_agent_id \
                                         (world_state.agent_id=None, api_state.agent_id=None)"
                                    );
                                } else {
                                    info!(
                                        "自动转世重生已调度: agent={}, delay={} ticks ({}s)",
                                        old_agent_id, delay_ticks, delay_ms / 1000
                                    );

                                    tokio::spawn(async move {
                                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                                        info!("自动转世重生: 调用 auto-rebirth API (old_agent={})", old_agent_id);

                                        let client = reqwest::Client::new();
                                        let url = format!("{}/api/v1/agent/auto-rebirth", http_url);
                                        let body = serde_json::json!({
                                            "device_id": device_id,
                                            "auth_token": auth_token,
                                            "old_agent_id": old_agent_id,
                                            "name": name,
                                            "system_prompt": system_prompt,
                                        });

                                        for attempt in 0..retry_max {
                                            match client.post(&url).json(&body).send().await {
                                                Ok(resp) if resp.status().is_success() => {
                                                    let data: serde_json::Value = resp.json().await.unwrap_or_default();
                                                    let new_id = data["new_agent_id"]
                                                        .as_str()
                                                        .and_then(|s| s.parse::<uuid::Uuid>().ok())
                                                        .unwrap_or(uuid::Uuid::nil());

                                                    info!(
                                                        "自动转世重生成功: old_agent={} → new_agent={}",
                                                        old_agent_id, new_id
                                                    );

                                                    if let Some(ref api_state) = api_state {
                                                        *api_state.pending_rebirth_agent_id.write().await = Some(new_id);
                                                        api_state.is_dead.store(false, std::sync::atomic::Ordering::Relaxed);
                                                        api_state.rebirth_notify.notify_waiters();
                                                    }
                                                    return;
                                                }
                                                Ok(resp) => {
                                                    let status = resp.status();
                                                    let resp_body = resp.text().await.unwrap_or_default();
                                                    warn!(
                                                        "自动转世重生服务端拒绝 (attempt {}/{}): status={}, body={}",
                                                        attempt + 1, retry_max, status, resp_body
                                                    );
                                                }
                                                Err(e) => {
                                                    warn!(
                                                        "自动转世重生网络错误 (attempt {}/{}): {}",
                                                        attempt + 1, retry_max, e
                                                    );
                                                }
                                            }
                                            if attempt + 1 < retry_max {
                                                tokio::time::sleep(retry_interval).await;
                                            }
                                        }
                                        tracing::error!(
                                            "自动转世重生最终失败: old_agent={}, 所有 {} 次重试用尽",
                                            old_agent_id, retry_max
                                        );
                                    });
                                }
                            }

                            continue;
                        }

                    // 1.5b 已死亡 → 跳过决策循环（等待重生恢复）
                    if self.death_reported {
                        if self.rebirth_delay_ticks > 0 {
                            let rebirth_done = self.http_api_state.as_ref()
                                .map(|s| !s.is_dead.load(std::sync::atomic::Ordering::Relaxed))
                                .unwrap_or(false);
                            if rebirth_done {
                                info!(
                                    "Agent '{}' 自动重生恢复决策: tick={}",
                                    self.character_name(),
                                    world_state.tick_id
                                );
                                self.death_reported = false;
                                self.death_tick_id = None;
                                if let Some(ref mut char_cfg) = self.character_config {
                                    char_cfg.status = crate::config::CharacterStatus::Alive;
                                    if let Some(ref api_state) = self.http_api_state {
                                        let characters_dir = api_state.character_dir.read().await.clone();
                                        if let Err(e) = save_character_config_to_fs(char_cfg, &characters_dir) {
                                            warn!("Failed to persist rebirth status: {}", e);
                                        }
                                    }
                                }
                            } else {
                                continue;
                            }
                        } else {
                            // 无自动重生：持续等待直到外部重生触发（通过 API 或重启）
                            continue;
                        }
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
                        // 即时事件不经过叙事合成（直接工作记忆）
                        if let Err(e) = self.process_events(&immediate_events, None).await {
                            warn!("即时事件写入记忆失败: {}", e);
                        }
                    }

                    // 2. 处理事件并更新记忆（叙事合成）
                    // 先 clone Arc 让 borrow 立即结束，避免与后续的 &mut self 冲突
                    let cognitive_engine_ref = self.cognitive_engine.as_ref().cloned();
                    if let Err(e) = self
                        .process_events(&world_state.events_log, cognitive_engine_ref.as_deref())
                        .await
                    {
                        warn!("Failed to process events into memory: {}", e);
                    }

                    // 2.5 社交事件 → 自动更新关系（非阻塞，spawn 后台任务）
                    self.process_social_events(&world_state.events_log, &world_state.entities);

                    // 3. 遗忘机制（间隔由 memory.forgetting_interval_ticks 配置）
                    if world_state.tick_id % self.config.memory.forgetting_interval_ticks == 0
                        && let Err(e) = self.run_forgetting(world_state.tick_id).await {
                            warn!("Failed to run forgetting mechanism: {}", e);
                        }

                    // 4. 构建增强的世界状态（包含记忆上下文 + 对话上下文）
                    let mut memory_context = self.get_memory_context().await;

                    // 4.x 拼接对话上下文
                    let dialogue_section = if let Some(ref dm) = self.dialogue_manager {
                        let guard = dm.read().await;
                        guard.get_active_sessions_context()
                    } else {
                        String::new()
                    };

                    if !dialogue_section.is_empty() {
                        memory_context = format!("{}\n\n# 活跃对话\n{}\n", memory_context, dialogue_section);
                    }

                    // 4.1 交易议价提示（经济引导，非生存干预）
                    // 附近有其他人且有银两时注入交易建议（关系感知）
                    let trade_hints = {
                        let mut hints = Vec::new();
                        let has_silver = world_state.self_state.inventory.iter()
                            .any(|i| i.item_id == "银子" && i.quantity > 0);
                        if !world_state.entities.is_empty() && has_silver {
                            let silver = world_state.self_state.inventory.iter()
                                .find(|i| i.item_id == "银子")
                                .map(|i| i.quantity)
                                .unwrap_or(0);

                            let mut entity_descs = Vec::new();
                            for entity in &world_state.entities {
                                let rel_desc = self.relationship_store
                                    .as_ref()
                                    .and_then(|store| store.get_relationship(entity.id).ok().flatten())
                                    .map(|rel| {
                                        let (_, label) = crate::component::social::get_relationship_level(rel.favorability);
                                        format!("{}（{}，好感度{}）", entity.name, label, rel.favorability)
                                    })
                                    .unwrap_or_else(|| format!("{}（陌生人，好感度0）", entity.name));
                                entity_descs.push(rel_desc);
                            }

                            hints.push(format!(
                                "【交易提示】你可以使用「说话」与对方讨价还价。先询价，协商好价格后再用「给予」交付物品换取银两。关系越好价格越优惠。你身边有：{}。你身上有{}两银子。",
                                entity_descs.join("、"),
                                silver,
                            ));
                        }
                        hints
                    };

                    // 4.2 Sanity：已移除天道干预式警告注入
                    // 低理智行为由 chaos generator (chaos.rs) 处理，体感叙事由
                    // WorldState.attribute_descriptions 提供（sanity 阈值 narrative_config.yaml）
                    if let Some(ref handler) = self.immediate_handler {
                        let store = handler.event_store();
                        let config = store.config();
                        let game_day = Self::compute_game_day(
                            &world_state.world_time,
                            self.config.game_rules.as_ref().and_then(|g| g.calendar.as_ref()),
                        );
                        match store.query_triaged_async(config.context.clone(), game_day).await {
                            Ok(triaged) => {
                                // URGENT: 逐条高可见性展示
                                for event in &triaged.urgent {
                                    let sender = event.from_agent_name.as_deref().unwrap_or("有人");
                                    memory_context.push_str(&format!(
                                        "\n!! 紧急事件: {}「{}」",
                                        sender, event.description
                                    ));
                                }

                                // BATCH: 摘要格式展示
                                if !triaged.batch.is_empty() {
                                    let batch_lines: Vec<String> = triaged.batch.iter()
                                        .take(config.context.max_batch_summary_chars / 20) // 粗略条目数限制
                                        .map(|e| {
                                            let sender = e.from_agent_name.as_deref().unwrap_or("有人");
                                            format!("- {}: {}", sender, e.description)
                                        })
                                        .collect();
                                    let batch_summary = batch_lines.join("\n");
                                    if !batch_summary.is_empty() {
                                        memory_context.push_str(&format!(
                                            "\n### 近期事件摘要\n{}\n",
                                            batch_summary
                                        ));
                                    }
                                }

                                // 标记已消费（按 ID，避免与后台 triage 竞态）
                                if !triaged.urgent.is_empty() || !triaged.batch.is_empty() {
                                    let consumed_ids: Vec<i64> = triaged.urgent.iter()
                                        .chain(triaged.batch.iter())
                                        .map(|e| e.id)
                                        .collect();
                                    if let Err(e) = store.mark_processed_by_ids_async(consumed_ids, world_state.tick_id).await {
                                        warn!("标记已消费事件失败: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("查询 triage 事件失败: {}", e);
                            }
                        }
                    }
                    if !memory_context.is_empty() {
                        debug!("Memory context:\n{}", memory_context);
                    }

                    // 4.5 天魂执行叙事生成（上一轮行动结果，用于 memory_context 和 soul_cycle_record 回填）
                    {
                        let last_intents = last_intents_for_narrative.lock().unwrap().clone();

                        // 数据驱动的上轮行动摘要：从 soul_cycle_recorder 读取上轮人魂叙事
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

                        // 上一轮行动结果注入 memory_context
                        if let Some(ref summary) = last_action_summary {
                            memory_context.push_str(&format!(
                                "\n### 上一轮行动结果\n{}\n",
                                summary
                            ));
                        }
                    }

                    // 4.3 交易议价提示注入
                    if !trade_hints.is_empty() {
                        memory_context.push_str("\n### 交易提示\n");
                        memory_context.push_str(&trade_hints.join("\n"));
                    }

                    // 4.4 托梦注入（统一路径：消费 dream 并注入 memory_context）
                    let active_dream: Option<String> = if let Some(ref api_state) = self.http_api_state
                        && let Some(dream_thought) = api_state.consume_dream().await
                    {
                        info!("[dream] 托梦注入决策上下文: {}字", dream_thought.chars().count());
                        memory_context.push_str("\n### 托梦\n");
                        memory_context.push_str(&dream_thought);
                        memory_context.push('\n');
                        Some(dream_thought)
                    } else {
                        None
                    };

                    // 4.4b 跨 Agent 传承教训注入
                    if !world_state.lessons_learned.is_empty() {
                        memory_context.push_str("\n### 前人教训\n");
                        for lesson in &world_state.lessons_learned {
                            memory_context.push_str(&format!("- {}\n", lesson.lesson));
                        }
                    }

                    // 4.5 决策上下文快照写入（供 /api/v1/context enrichment 使用）
                    if let Some(ref api_state) = self.http_api_state {
                        let (summary_ctx, outcome_ctx, action_desc, action_hints) =
                            if let Some(ref engine) = self.cognitive_engine {
                                let (desc, hints) = engine.get_action_context();
                                (
                                    engine.get_summary_context(),
                                    engine.get_outcome_context_public(),
                                    desc,
                                    hints,
                                )
                            } else {
                                (String::new(), String::new(), String::new(), String::new())
                            };

                        // 读取上次执行结果（如果有）
                        let last_exec = api_state.decision_context_snapshot.read().await
                            .as_ref()
                            .and_then(|s| s.last_execution_result.clone());

                        let snapshot = crate::infra::api::DecisionContextSnapshot {
                            tick_id: world_state.tick_id,
                            memory_context: memory_context.clone(),
                            summary_context: summary_ctx,
                            outcome_section: outcome_ctx,
                            action_descriptions: action_desc,
                            action_field_hints: action_hints,
                            last_execution_result: last_exec,
                        };
                        *api_state.decision_context_snapshot.write().await = Some(snapshot);
                    }

                    // 5. 三魂循环：generate -> validate -> self_correct once -> chaos_fallback
                    // Token 优化：消灭 13 轮重试循环，固定为最多 2 次 LLM 调用
                    // 提前提取优化配置（避免后续 borrow 冲突）
                    let (opt_enabled, opt_self_correction, opt_chaos_on_double_reject, opt_chaos_on_llm_fail) = {
                        let opt = &self.config.token_optimization;
                        (opt.enabled, opt.reflector.self_correction, opt.reflector.chaos_on_double_reject, opt.reflector.chaos_on_llm_fail)
                    };
                    let max_retries: i32 = if opt_enabled {
                        // 优化模式：最多 self_correct 一次（attempt 0=初始, 1=纠正）
                        1
                    } else {
                        // 旧模式：保留原有重试上限
                        self.config.game_rules
                            .as_ref()
                            .and_then(|g| g.intent_batch.as_ref())
                            .map(|b| b.max_retries)
                            .unwrap_or(12)
                    };
                    let _max_intents = self.config.game_rules
                        .as_ref()
                        .and_then(|g| g.intent_batch.as_ref())
                        .map(|b| b.max_intents_per_tick)
                        .unwrap_or(5);
                    let agent_id = world_state.agent_id.unwrap_or_default();
                    let mut final_intent = None;
                    let mut final_intent_validated = false;

                    // tick 级 LLM 失败计数器（优化模式下使用）
                    let mut tick_llm_fail_count: u32 = 0;

                    for attempt in 0..=max_retries {

                        // 5a. 人魂 (ActorSoul) 决策 — 直连 WorldState，输出结构化 Intent
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

                        // 记录人魂输出（可读简述）
                        let renhun_narrative = Self::summarize_intent(
                            raw_intent.action_type.as_str(),
                            raw_intent.action_data.as_ref(),
                            &world_state.location.name,
                        );
                        let renhun_thought_log = raw_intent.thought_log.as_deref().unwrap_or("");
                        if let Some(recorder) = self.soul_recorder().await {
                            recorder.record_renhun(
                                world_state.tick_id,
                                attempt,
                                &renhun_narrative,
                                renhun_thought_log,
                            ).await;
                            let world_time_str = Self::format_world_time(&world_state.world_time);
                            recorder.record_world_time(world_state.tick_id, attempt, &world_time_str).await;
                        } else {
                            tracing::warn!(
                                "[soul_cycle] recorder unavailable at tick {}, skipping renhun record",
                                world_state.tick_id
                            );
                        }

                        // 5c. 天魂 (ReflectorSoul) 审核 — 分级审核策略
                        let graded_config = self.config.game_rules
                            .as_ref()
                            .and_then(|g| g.intent_batch.as_ref())
                            .map(|b| b.llm_validation.clone());

                        let mut approved_intents = Vec::new();
                        let mut batch_rejection: Option<String> = None;
                        let mut batch_layers: Vec<crate::soul::reflector::LayerResult> = Vec::new();
                        let mut batch_narrative: Option<String> = None;

                        // multi-intent pipeline: primary + subsequent intents + chaos
                        let max_per_tick = _max_intents;
                        let mut all_raw_intents: Vec<Intent> = {
                            let mut intents: Vec<Intent> = if self.llm_chaos_active {
                                Vec::new()
                            } else {
                                vec![raw_intent.clone()]
                            };
                            if let Some(ref chain) = _cognitive_chain
                                && let Some(ref multi) = chain.multi_intents
                            {
                                for i in multi.iter().take(max_per_tick.saturating_sub(1)) {
                                    intents.push(i.clone());
                                }
                            }
                            if let Some(ref mut generator) = self.chaos_generator {
                                let remaining = max_per_tick.saturating_sub(intents.len());
                                if remaining > 0 {
                                    let actions: Vec<_> = self.config.game_rules
                                        .as_ref()
                                        .map(|g| g.available_actions.clone())
                                        .unwrap_or_default();
                                    let chaos_intents = generator.generate_chaos_intents(&world_state, &actions, remaining);
                                    intents.extend(chaos_intents);
                                }
                            }
                            if self.llm_chaos_active
                                && let Some(ref mut generator) = self.chaos_generator
                            {
                                let remaining = max_per_tick.saturating_sub(intents.len());
                                if remaining > 0 {
                                    let actions: Vec<_> = self.config.game_rules
                                        .as_ref()
                                        .map(|g| g.available_actions.clone())
                                        .unwrap_or_default();
                                    let llm_chaos = generator.generate_llm_chaos_intents(&world_state, &actions, remaining, self.consecutive_llm_failures as usize);
                                    tracing::info!("LLM chaos: generated {} intents from {} actions", llm_chaos.len(), actions.len());
                                    intents.extend(llm_chaos);
                                }
                            }
                            intents
                        };

                        // 托梦标记
                        if let Some(ref dream) = active_dream {
                            let dream_trunc = self
                                .cognitive_engine
                                .as_ref()
                                .map(|e| e.truncation("dream_marker_thought", 50))
                                .unwrap_or(50);
                            let summary: String = dream.chars().take(dream_trunc).collect();
                            for intent in &mut all_raw_intents {
                                intent.dream_marker = Some(
                                    cyber_jianghu_protocol::types::DreamMarker {
                                        thought: summary.clone(),
                                    },
                                );
                            }
                        }

                        // 重要记忆固化
                        #[allow(clippy::collapsible_if)]
                        if let Some(ref chain) = _cognitive_chain
                            && chain.should_remember == Some(true)
                            && let Some(ref content) = chain.memory_content
                            && let Some(ref mm) = self.memory_manager
                        {
                                let entry = crate::component::memory::types::MemoryEntry::new(
                                    world_state.agent_id.unwrap_or_default(),
                                    world_state.tick_id,
                                    content.clone(),
                                )
                                .with_importance(1.0);
                                let mut mm_guard = mm.write().await;
                                if let Err(e) = mm_guard.episodic_mut().add(&mut entry.clone()).await {
                                    warn!("重要记忆固化失败: {}", e);
                                } else {
                                    info!("重要记忆已固化: {}", content);
                                }
                        }

                        // 逐 intent 审查 + self-correction（优化模式）
                        for intent in all_raw_intents {
                            match self
                                .validate_with_reflector(intent, &world_state, graded_config.as_ref())
                                .await?
                            {
                                crate::soul::reflector::PipelineValidationResult::Approved {
                                    intent: approved,
                                    layers,
                                    narrative,
                                } => {
                                    batch_layers = layers;
                                    batch_narrative = narrative;
                                    approved_intents.push(approved);
                                }
                                crate::soul::reflector::PipelineValidationResult::Rejected {
                                    reason,
                                    layers,
                                } => {
                                    batch_layers = layers;
                                    let rejection_reason = reason.clone();
                                    self.set_rejection_feedback(reason.clone());
                                    warn!(
                                        "Tick {} attempt {} 天魂审查驳回: {}",
                                        world_state.tick_id, attempt, rejection_reason
                                    );

                                    // 优化模式：self-correct 一次后直接 chaos_fallback
                                    if opt_enabled
                                        && opt_self_correction
                                        && tick_llm_fail_count < opt_chaos_on_llm_fail
                                    {
                                        match self.self_correct_intent(
                                            &world_state, &memory_context, &rejection_reason,
                                        ).await {
                                            Ok(corrected_intent) => {
                                                match self.validate_with_reflector(
                                                    corrected_intent, &world_state, graded_config.as_ref(),
                                                ).await? {
                                                    crate::soul::reflector::PipelineValidationResult::Approved {
                                                        intent: approved, layers: l2, narrative: n2,
                                                    } => {
                                                        batch_layers = l2;
                                                        batch_narrative = n2;
                                                        approved_intents.push(approved);
                                                    }
                                                    crate::soul::reflector::PipelineValidationResult::Rejected {
                                                        reason: reason2, ..
                                                    } => {
                                                        warn!(
                                                            "Tick {} self-correct 后仍被驳回: {}",
                                                            world_state.tick_id, reason2
                                                        );
                                                        if opt_chaos_on_double_reject {
                                                            approved_intents.push(
                                                                self.chaos_fallback_intent(
                                                                    &world_state, agent_id,
                                                                    format!("self-correct 后仍被驳回: {}", reason2),
                                                                )
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                tick_llm_fail_count += 1;
                                                warn!(
                                                    "Tick {} self-correct LLM 失败 ({}): {}",
                                                    world_state.tick_id, tick_llm_fail_count, e
                                                );
                                                approved_intents.push(
                                                    self.chaos_fallback_intent(
                                                        &world_state, agent_id,
                                                        format!("self-correct LLM 失败: {}", e),
                                                    )
                                                );
                                            }
                                        }
                                    } else if opt_enabled && opt_chaos_on_double_reject {
                                        approved_intents.push(
                                            self.chaos_fallback_intent(
                                                &world_state, agent_id,
                                                format!("意图被驳回（跳过 self-correct）: {}", rejection_reason),
                                            )
                                        );
                                    } else {
                                        // 旧模式：记录 batch_rejection 以触发重试
                                        batch_rejection = Some(rejection_reason);
                                    }
                                }
                            }

                            // 旧模式：primary intent 被驳回则终止批次（Pipeline 语义）
                            if !opt_enabled && batch_rejection.is_some() {
                                break;
                            }
                        }

                        if !approved_intents.is_empty() {
                            if let Some(recorder) = self.soul_recorder().await {
                                let layer1 = batch_layers.iter().find(|l| l.layer == "layer1");
                                let layer2 = batch_layers.iter().find(|l| l.layer == "layer2");
                                let layer3 = batch_layers.iter().find(|l| l.layer == "layer3");
                                recorder.record_tianhun(
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
                                final_intent_validated = true;
                            } else {
                                let pipeline = Self::assemble_pipeline(approved_intents.clone());
                                final_intent = Some(pipeline);
                                final_intent_validated = true;
                            }
                            if let Ok(mut saved) = last_intents_for_narrative.lock() {
                                saved.clone_from(&approved_intents);
                            } else {
                                warn!("暂存 approved_intents 失败：Mutex lock 获取失败");
                            }
                            break;
                        } else if let Some(reason) = batch_rejection.clone() {
                            // 仅旧模式会进入此分支
                            if let Some(recorder) = self.soul_recorder().await {
                                let layer1 = batch_layers.iter().find(|l| l.layer == "layer1");
                                let layer2 = batch_layers.iter().find(|l| l.layer == "layer2");
                                let layer3 = batch_layers.iter().find(|l| l.layer == "layer3");
                                let narrated = super::Agent::narrativize_rejection(&reason);
                                recorder.record_tianhun(
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
                                warn!("Tick {} 达到最大重试次数 {}，使用 chaos fallback", world_state.tick_id, max_retries);
                                final_intent = Some(self.chaos_fallback_intent(&world_state, agent_id, format!("意图多次被驳回: {}", reason)));
                                break;
                            }
                        }
                    }

                    let mut final_intent = match final_intent {
                        Some(intent) => intent,
                        None => {
                            warn!("Tick {} 无有效 intent（超时或被驳回耗尽），使用 chaos fallback", world_state.tick_id);
                            self.consecutive_idle_count += 1;
                            self.consecutive_follow_count = 0;
                            self.maybe_rotate_model().await;
                            self.chaos_fallback_intent(&world_state, agent_id, "三魂循环未产出有效意图".to_string())
                        }
                    };

                    // 后置 chaos 替换：如果 callback（engine/binary 层）返回了认知失败标记的休息，
                    // 用 chaos 生存 intent 替换，避免"认知失败 → 固定休息 → 饿死"死循环
                    if final_intent.action_type.as_str() == "休息"
                        && final_intent.thought_log.as_ref()
                            .map(|t| t.contains("认知失败") || t.contains("忽然心神不宁"))
                            .unwrap_or(false)
                    {
                        let chaos_intent = self.chaos_fallback_intent(
                            &world_state, agent_id,
                            final_intent.thought_log.clone().unwrap_or_default(),
                        );
                        info!(
                            "认知失败休息 → chaos 替换: action={}",
                            chaos_intent.action_type
                        );
                        final_intent = chaos_intent;
                    }

                    // LLM 失败追踪：检测是否为 LLM 不可用导致的 fallback（chaos 或 idle）
                    let is_llm_failure = final_intent.chaos_marker.is_some()
                        || final_intent.thought_log.as_ref()
                            .map(|t| t.contains("意图多次被驳回")
                                || t.contains("三魂循环未产出有效意图")
                                || t.contains("认知失败")
                                || t.contains("[LLM 配额耗尽"))
                            .unwrap_or(false);
                    if is_llm_failure {
                        self.consecutive_llm_failures += 1;
                    } else {
                        self.consecutive_llm_failures = 0;
                    }
                    // 阈值: 从 game_rules 读取 llm_chaos_threshold（默认 12）
                    let llm_chaos_threshold = self.config.game_rules
                        .as_ref()
                        .and_then(|g| g.intent_batch.as_ref())
                        .map(|b| b.llm_chaos_threshold)
                        .unwrap_or(12);
                    let was_chaos_active = self.llm_chaos_active;
                    self.llm_chaos_active = self.consecutive_llm_failures >= llm_chaos_threshold;
                    if self.llm_chaos_active && !was_chaos_active {
                        warn!(
                            "LLM chaos 模式激活: agent={}, consecutive_failures={}",
                            self.character_name(), self.consecutive_llm_failures
                        );
                    } else if !self.llm_chaos_active && was_chaos_active {
                        info!("LLM chaos 模式解除: agent={}, LLM 恢复正常", self.character_name());
                    }

                    // 5.6 记录 Intent 到经历日志（供 Web Panel 查询）
                    if let Some(ref api_state) = self.http_api_state
                        && let Some(history) = api_state.intent_history.read().await.as_ref() {
                            history
                                .record_intent(
                                    final_intent.tick_id,
                                    0,
                                    final_intent.intent_id,
                                    final_intent.action_type.to_string(),
                                    final_intent.thought_log.clone(),
                                )
                                .await;
                        }

                    let graded_config = self.config.game_rules
                        .as_ref()
                        .and_then(|g| g.intent_batch.as_ref())
                        .map(|b| b.llm_validation.clone());

                    // 6. 发送意图
                    if !final_intent_validated {
                        match self
                            .validate_with_reflector(
                                final_intent.clone(),
                                &world_state,
                                graded_config.as_ref(),
                            )
                            .await?
                        {
                            crate::soul::reflector::PipelineValidationResult::Approved {
                                intent: approved,
                                ..
                            } => {
                                final_intent = approved;
                            }
                            crate::soul::reflector::PipelineValidationResult::Rejected { reason, .. } => {
                                self.set_rejection_feedback(reason.clone());
                                warn!(
                                    "Tick {} fallback intent 被天魂驳回，改用 chaos fallback: {}",
                                    world_state.tick_id, reason
                                );
                                final_intent = self.chaos_fallback_intent(
                                    &world_state,
                                    agent_id,
                                    format!("fallback 被天魂驳回: {}", reason),
                                );
                            }
                        }
                    }

                    if let Err(e) = self.client.send_intent(&final_intent).await {
                            error!("Failed to send intent: {}", e);
                            if let Err(reconnect_err) = self.reconnect().await {
                                error!("Reconnect failed: {}", reconnect_err);
                            }
                        } else {
                            info!(
                                "Intent sent successfully: tick={}, action={}, agent={}",
                                final_intent.tick_id, final_intent.action_type, final_intent.agent_id
                            );

                            // 记录 Agent 自身对话消息（原子化：write lock 内解析 session_id）
                            {
                                use crate::component::dialogue::DialogueRole;
                                if let Some(ref dm) = self.dialogue_manager {
                                    let action_type_str = final_intent.action_type.as_str();
                                    let is_dialogue = {
                                        let dm = dm.read().await;
                                        dm.is_dialogue_action(action_type_str)
                                    };
                                    if is_dialogue {
                                        let content = final_intent.action_data
                                            .as_ref()
                                            .and_then(|d| d.get("content"))
                                            .and_then(|c| c.as_str())
                                            .unwrap_or("");
                                        if !content.is_empty() {
                                            let target_id = final_intent.action_data
                                                .as_ref()
                                                .and_then(|d| d.get("target_agent_id"))
                                                .and_then(|t| t.as_str())
                                                .and_then(|s| Uuid::parse_str(s).ok());
                                            let tick = self.current_tick.load(std::sync::atomic::Ordering::Relaxed);
                                            let mut guard = dm.write().await;
                                            let session_id = if let Some(tid) = target_id {
                                                guard.get_session_id_by_partner(&tid)
                                                    .map(|s| s.to_string())
                                                    .unwrap_or_else(|| format!("{}{}", crate::component::dialogue::PENDING_SESSION_PREFIX, tid))
                                            } else {
                                                format!("speak_{}", chrono::Utc::now().timestamp())
                                            };
                                            guard.add_message(
                                                &session_id,
                                                final_intent.agent_id,
                                                DialogueRole::Own,
                                                content,
                                                tick,
                                            );
                                        }
                                    }
                                }
                            }

                            // 实时模式：等待 ExecutionResult（server 立即处理后的反馈）
                            // Server 同时会发送 reactive WorldState（交互驱动即时推送），
                            // 下一次 select 循环的 receive_world_state() 会立即收到（无需等 tick 广播）。
                            // 使用 watch channel 阻塞等待，3s 超时（替代固定 sleep + 非阻塞 poll）
                            match self.client.wait_for_execution_result(self.config.llm.execution_result_timeout_ms).await {
                                Ok(Some(result)) => {
                                    // 快照数据提取（在分支消费 result 之前）
                                    let exec_success = result.success;
                                    let exec_error = result.error.clone();

                                    if result.success {
                                        debug!(
                                            "ExecutionResult: tick={}, intent={}, success",
                                            result.tick_id, result.intent_id
                                        );
                                        // Outcome 写回：更新 summary window
                                        if let Some(ref engine) = self.cognitive_engine {
                                            engine.update_summary_outcome(format!("成功: {}", final_intent.action_type));
                                        }
                                        // Outcome Memory 记录成功经验
                                        if let Some(ref engine) = self.cognitive_engine {
                                            engine.record_outcome(crate::component::memory::OutcomeRecord {
                                                action_type: final_intent.action_type.to_string(),
                                                action_data: final_intent.action_data.clone(),
                                                result: crate::component::memory::OutcomeResult::Success,
                                                context_hash: crate::component::memory::compute_context_hash(&world_state),
                                                tick_id: final_intent.tick_id,
                                            });
                                        }
                                    } else {
                                        warn!(
                                            "ExecutionResult: tick={}, intent={}, FAILED: {}",
                                            result.tick_id,
                                            result.intent_id,
                                            result.error.as_deref().unwrap_or("unknown")
                                        );
                                        // BUG-4b: intent 失败且 agent 已死亡 → 立即触发死亡处理
                                        // 场景：认知循环进行中收到 AgentDied WS 回调，is_dead=true，
                                        // 认知完成后提交 intent 被拒绝。此时 DashMap 已移除 dead agent，
                                        // 后续 WorldState 永不到达，tick 循环将挂起。
                                        if !self.death_reported {
                                            let is_dead_now = self.http_api_state.as_ref()
                                                .map(|s| s.is_dead.load(std::sync::atomic::Ordering::Relaxed))
                                                .unwrap_or(false);
                                            if is_dead_now {
                                                let reason_str = result.error.as_deref().unwrap_or("");
                                                warn!(
                                                    "Agent '{}' 检测到死亡（intent 失败后）: {}",
                                                    self.character_name(), reason_str
                                                );
                                                self.death_reported = true;
                                                self.death_tick_id = Some(world_state.tick_id);

                                                // 读取 rebirth_delay_ticks（WS 回调已写入）
                                                if let Some(ref api_state) = self.http_api_state {
                                                    self.rebirth_delay_ticks = api_state
                                                        .rebirth_delay_ticks.load(std::sync::atomic::Ordering::Relaxed);
                                                }

                                                // 持久化死亡状态
                                                if let Some(ref mut char_cfg) = self.character_config {
                                                    char_cfg.status = crate::config::CharacterStatus::Dead;
                                                    if let Some(ref api_state) = self.http_api_state {
                                                        let characters_dir = api_state.character_dir.read().await.clone();
                                                        if let Err(e) = save_character_config_to_fs(char_cfg, &characters_dir) {
                                                            warn!("Failed to persist death status: {}", e);
                                                        }
                                                    }
                                                }

                                                // 调度 auto-rebirth（转世重生，含重试）
                                                let auto_rebirth_enabled = self.http_api_state
                                                    .as_ref()
                                                    .map(|s| s.auto_rebirth.load(std::sync::atomic::Ordering::Relaxed))
                                                    .unwrap_or(true);
                                                if self.rebirth_delay_ticks > 0 && auto_rebirth_enabled {
                                                    let delay_ticks = self.rebirth_delay_ticks;
                                                    let tick_secs = self.get_tick_duration().await.as_secs();
                                                    let delay_ms = delay_ticks as u64 * tick_secs * 1000;
                                                    let old_agent_id = world_state.agent_id.unwrap_or_default();
                                                    let http_url = self.config.server.http_url.clone();
                                                    let api_state = self.http_api_state.clone();

                                                    if let Some(device_cfg) = self.device_config.as_ref() {
                                                        let device_id = device_cfg.device_id;
                                                        let auth_token = device_cfg.auth_token.clone();
                                                        // 复用旧角色 name + system_prompt（转世：同角色新 agent_id）
                                                        let (name, system_prompt) = self.character_config
                                                            .as_ref()
                                                            .map(|cc| (cc.name.clone(), cc.system_prompt.clone().unwrap_or_default()))
                                                            .unwrap_or_default();
                                                    // 重试参数（数据驱动，从 GameRules 配置读取）
                                                    let retry_max = self.config.game_rules
                                                        .as_ref()
                                                        .map(|r| r.rebirth_retry_max_attempts)
                                                        .unwrap_or(3);
                                                    let retry_interval = std::time::Duration::from_secs(
                                                        self.config.game_rules
                                                            .as_ref()
                                                            .map(|r| r.rebirth_retry_interval_secs)
                                                            .unwrap_or(30)
                                                    );

                                                    info!(
                                                        "自动转世重生已调度（intent失败路径）: agent={}, delay={} ticks ({}s)",
                                                        old_agent_id, delay_ticks, delay_ms / 1000
                                                    );

                                                    tokio::spawn(async move {
                                                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                                                        info!("自动转世重生（intent失败路径）: old_agent={}", old_agent_id);

                                                        let client = reqwest::Client::new();
                                                        let url = format!("{}/api/v1/agent/auto-rebirth", http_url);
                                                        let body = serde_json::json!({
                                                            "device_id": device_id,
                                                            "auth_token": auth_token,
                                                            "old_agent_id": old_agent_id,
                                                            "name": name,
                                                            "system_prompt": system_prompt,
                                                        });

                                                        for attempt in 0..retry_max {
                                                            match client.post(&url).json(&body).send().await {
                                                                Ok(resp) if resp.status().is_success() => {
                                                                    let data: serde_json::Value = resp.json().await.unwrap_or_default();
                                                                    let new_id = data["new_agent_id"]
                                                                        .as_str()
                                                                        .and_then(|s| s.parse::<uuid::Uuid>().ok())
                                                                        .unwrap_or(uuid::Uuid::nil());

                                                                    info!(
                                                                        "自动转世重生成功: old_agent={} → new_agent={}",
                                                                        old_agent_id, new_id
                                                                    );
                                                                    if let Some(ref api_state) = api_state {
                                                                        *api_state.pending_rebirth_agent_id.write().await = Some(new_id);
                                                                        api_state.is_dead.store(false, std::sync::atomic::Ordering::Relaxed);
                                                                        api_state.rebirth_notify.notify_waiters();
                                                                    }
                                                                    return;
                                                                }
                                                                Ok(resp) => {
                                                                    let status = resp.status();
                                                                    warn!(
                                                                        "自动转世重生服务端拒绝 (attempt {}/{}): status={}",
                                                                        attempt + 1, retry_max, status
                                                                    );
                                                                }
                                                                Err(e) => {
                                                                    warn!(
                                                                        "自动转世重生网络错误 (attempt {}/{}): {}",
                                                                        attempt + 1, retry_max, e
                                                                    );
                                                                }
                                                            }
                                                            if attempt + 1 < retry_max {
                                                                tokio::time::sleep(retry_interval).await;
                                                            }
                                                        }
                                                        tracing::error!(
                                                            "自动转世重生最终失败（intent失败路径）: old_agent={}, 所有 {} 次重试用尽",
                                                            old_agent_id, retry_max
                                                        );
                                                    });
                                                    }
                                                }
                                            }
                                        }
                                        // 注入失败原因到下轮推理上下文
                                        let reason = result.error.unwrap_or_default();
                                        {
                                            let mut guard = self.server_error_feedback.lock().await;
                                            *guard = Some(format!("[意图执行失败: {}]", reason));
                                        }
                                        // Outcome 写回：更新 summary window
                                        if let Some(ref engine) = self.cognitive_engine {
                                            engine.update_summary_outcome(format!("失败: {}", reason));
                                        }
                                        // Outcome Memory 记录失败经验
                                        if let Some(ref engine) = self.cognitive_engine {
                                            engine.record_outcome(crate::component::memory::OutcomeRecord {
                                                action_type: final_intent.action_type.to_string(),
                                                action_data: final_intent.action_data.clone(),
                                                result: crate::component::memory::OutcomeResult::Failed(reason.clone()),
                                                context_hash: crate::component::memory::compute_context_hash(&world_state),
                                                tick_id: final_intent.tick_id,
                                            });
                                        }
                                    }

                                    // 更新执行结果到快照（供 /api/v1/context enrichment 使用）
                                    if let Some(ref api_state) = self.http_api_state {
                                        let mut snapshot = api_state.decision_context_snapshot.write().await;
                                        if let Some(s) = snapshot.as_mut() {
                                            s.last_execution_result = Some(
                                                crate::infra::api::ExecutionSummary {
                                                    action_type: final_intent.action_type.to_string(),
                                                    success: exec_success,
                                                    narrative: exec_error.unwrap_or_default(),
                                                }
                                            );
                                        }
                                    }
                                }
                                Ok(None) => {
                                    debug!("ExecutionResult timeout (3s), server may be slow");
                                }
                                Err(e) => {
                                    debug!("ExecutionResult poll error: {}", e);
                                }
                            }

                            if final_intent.action_type.as_str() != "休息" {
                                self.consecutive_idle_count = 0;
                                // 连续 follow 计数（社交死循环防护）
                                if final_intent.action_type.as_str() == "follow" {
                                    self.consecutive_follow_count += 1;
                                } else {
                                    self.consecutive_follow_count = 0;
                                }
                                if let Some(ref container) = self.actor_llm_container {
                                    let llm = container.read().await;
                                    llm.reset_idle_count();
                                }
                            }
                            if final_intent.action_type.as_str() == "休息" {
                                self.maybe_rotate_model().await;
                            }

                            // 7.5 上报三魂循环元数据到服务器（使 server-web 可见）
                            let tick_id_for_report = final_intent.tick_id;
                            if let Some(recorder) = self.soul_recorder().await {
                                let records = recorder.get_by_tick(tick_id_for_report).await;
                                let immediate_records = recorder.get_immediate_by_tick(tick_id_for_report).await;

                                let world_time = records.first().and_then(|r| r.world_time.clone());

                                let cycles: Vec<cyber_jianghu_protocol::SoulCycleAttempt> = records.into_iter().map(|r| {
                                    let layers: Vec<cyber_jianghu_protocol::LayerReport> = vec![
                                        (r.tianhun_layer1_result.as_deref(), "layer1"),
                                        (r.tianhun_layer2_result.as_deref(), "layer2"),
                                        (r.tianhun_layer3_result.as_deref(), "layer3"),
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
                                            result: r.tianhun_result,
                                            layers,
                                            reason: r.tianhun_reason,
                                            narrative: r.previous_round_narrative,
                                        },
                                        final_intent: r.final_intent_id.map(|id| cyber_jianghu_protocol::FinalIntentReport {
                                            intent_id: Some(id),
                                            action_type: r.final_action_type.clone(),
                                            action_data: r.final_action_data.as_ref().and_then(|s| serde_json::from_str(s).ok()),
                                            // markers 存于 Server DB agent_action_logs（processor.rs 提取），此处 SoulCycleRecord 不存储
                                            chaos_marker: None,
                                            dream_marker: None,
                                        }),
                                    }
                                }).collect();

                                let agent_name = self.character_name().to_string();
                                let immediate_intents: Vec<cyber_jianghu_protocol::ImmediateIntentReport> = immediate_records.into_iter().map(|r| {
                                    cyber_jianghu_protocol::ImmediateIntentReport {
                                        intent_id: r.intent_id,
                                        route_type: r.route_type,
                                        action_type: r.action_type,
                                        action_data: r.action_data.as_ref().and_then(|s| serde_json::from_str(s).ok()),
                                        from_agent_name: Some(agent_name.clone()),
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
                                let max_retries = self.config.llm.soul_cycle_report_retries;
                                let base_delay = self.config.llm.soul_cycle_report_base_delay_ms;
                                for attempt in 0..max_retries {
                                    match self.client.send_soul_cycle_report(tick_id_for_report, 0, metadata.clone()).await {
                                        Ok(()) => {
                                            debug!("三魂循环元数据上报成功: tick={}", tick_id_for_report);
                                            reported = true;
                                            break;
                                        }
                                        Err(e) => {
                                            warn!("三魂循环元数据上报失败 (尝试 {}/{}): tick={}, err={}", attempt + 1, max_retries, tick_id_for_report, e);
                                            if attempt + 1 < max_retries {
                                                tokio::time::sleep(tokio::time::Duration::from_millis(base_delay * (1 << attempt))).await;
                                            }
                                        }
                                    }
                                }
                                if !reported {
                                    error!("三魂循环元数据上报最终失败: tick={}", tick_id_for_report);
                                }

                                // 7.6 上报后续 intent 的简化元数据（subsequent intents 不经过三魂审查）
                                let subsequent_count = final_intent.subsequent_intents.len();
                                if subsequent_count > 0 {
                                    let world_time = metadata.world_time.clone();
                                    for (idx, subsequent) in final_intent.subsequent_intents.iter().enumerate() {
                                        let pipe_seq = (idx + 1) as i32;
                                        // Subsequent intents bypass soul review: simplified metadata with single passed cycle
                                        let simplified_metadata = cyber_jianghu_protocol::SoulCycleMetadata {
                                            world_time: world_time.clone(),
                                            cycles: vec![cyber_jianghu_protocol::SoulCycleAttempt {
                                                attempt: 0,
                                                renhun: cyber_jianghu_protocol::RenhunReport {
                                                    narrative: Some("后续意图".to_string()),
                                                    thought_log: None,
                                                },
                                                tianhun: cyber_jianghu_protocol::TianhunReport {
                                                    result: Some("通过".to_string()),
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
                                                    narrative: Some(format!("后续动作: {}", subsequent.action_type)),
                                                },
                                                final_intent: Some(cyber_jianghu_protocol::FinalIntentReport {
                                                    intent_id: Some(subsequent.intent_id.to_string()),
                                                    action_type: Some(subsequent.action_type.to_string()),
                                                    action_data: subsequent.action_data.clone(),
                                                    chaos_marker: subsequent.chaos_marker.clone(),
                                                    dream_marker: subsequent.dream_marker.clone(),
                                                }),
                                            }],
                                            immediate_intents: vec![],
                                        };

                                        let mut reported = false;
                                        for attempt in 0..max_retries {
                                            match self.client.send_soul_cycle_report(tick_id_for_report, pipe_seq, simplified_metadata.clone()).await {
                                                Ok(()) => {
                                                    debug!("后续意图元数据上报成功: tick={}, pipe_seq={}, action={}", tick_id_for_report, pipe_seq, subsequent.action_type);
                                                    reported = true;
                                                    break;
                                                }
                                                Err(e) => {
                                                    warn!("后续意图元数据上报失败 (尝试 {}/{}): tick={}, pipe_seq={}, err={}", attempt + 1, max_retries, tick_id_for_report, pipe_seq, e);
                                                    if attempt + 1 < max_retries {
                                                        tokio::time::sleep(tokio::time::Duration::from_millis(base_delay * (1 << attempt))).await;
                                                    }
                                                }
                                            }
                                        }
                                        if !reported {
                                            error!("后续意图元数据上报最终失败: tick={}, pipe_seq={}", tick_id_for_report, pipe_seq);
                                        }
                                    }
                                }
                            }

                            // 8. 对话历史 summary 压缩（长窗口）
                            if let Some(ref engine) = self.cognitive_engine
                                && engine.conversation_needs_summary()
                                && let Some(prompt) = engine.conversation_summary_prompt()
                                && let Some(ref container) = self.actor_llm_container
                            {
                                let llm = container.read().await;
                                if let Ok(summary) = llm.complete(&prompt).await {
                                    engine.conversation_replace_with_summary(summary);
                                    info!("对话历史 summary 压缩完成");
                                } else {
                                    tracing::warn!("对话历史 summary 生成失败，降级为强制截断");
                                    engine.conversation_force_truncate();
                                }
                            }
                        }
                }
            }

            // 每个 tick 结束时持久化 token 统计
            crate::component::llm::token_tracking::persist_and_reset();
        }
    }

    /// 发送即时 Intent（统一走主 intent 通道）
    #[allow(dead_code)]
    async fn send_immediate_intent(&self, intent: &Intent) -> std::result::Result<(), String> {
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

    /// 从 WorldTime 计算游戏日（用于 EventStore game_day 字段）
    ///
    /// 数据驱动：从 CalendarConfig (time.yaml) 读取 days_per_season / seasons_per_year。
    /// game_day = (year-1) * days_per_year + (month-1) * days_per_season + day
    /// 其中 days_per_year = seasons_per_year * days_per_season
    fn compute_game_day(time: &WorldTime, calendar: Option<&CalendarConfig>) -> i64 {
        if let Some(cal) = calendar {
            let days_per_year = cal.seasons_per_year as i64 * cal.days_per_season as i64;
            (time.year as i64 - 1) * days_per_year
                + (time.month as i64 - 1) * cal.days_per_season as i64
                + time.day as i64
        } else {
            // 降级：无 calendar 配置时（旧服务器），用单调排序键避免碰撞
            (time.year as i64) * 10000 + (time.month as i64) * 100 + time.day as i64
        }
    }

    pub async fn close(&mut self) -> Result<()> {
        // 终止 SessionTriageEngine 后台任务
        if let Some(handle) = self.session_triage_handle.take() {
            handle.abort();
        }
        self.client.close().await;
        info!("Agent '{}' stopped", self.character_name());
        Ok(())
    }

    /// 格式化游戏内时间（WorldTime → 中文武侠风格字符串）
    fn format_world_time(wt: &WorldTime) -> String {
        wt.to_chinese()
    }

    /// LLM 失败时的 chaos fallback：尝试生成生存导向 intent，失败则退回休息
    fn chaos_fallback_intent(
        &mut self,
        world_state: &cyber_jianghu_protocol::WorldState,
        agent_id: Uuid,
        fallback_thought: String,
    ) -> Intent {
        if let Some(ref mut generator) = self.chaos_generator {
            let actions: Vec<_> = self
                .config
                .game_rules
                .as_ref()
                .map(|g| g.available_actions.clone())
                .unwrap_or_default();
            if !actions.is_empty() {
                let chaos_intents = generator.generate_llm_chaos_intents(
                    world_state,
                    &actions,
                    1,
                    self.consecutive_llm_failures as usize,
                );
                if let Some(intent) = chaos_intents.into_iter().next() {
                    info!(
                        "Chaos fallback: agent={}, action={}",
                        self.character_name(),
                        intent.action_type
                    );
                    return intent;
                }
            }
        }
        // chaos 不可用 → 绝对兜底休息
        warn!(
            "Chaos fallback 不可用，退回休息: agent={}",
            self.character_name()
        );
        Intent::new(agent_id, world_state.tick_id, "休息", None).with_thought(fallback_thought)
    }

    /// 将 action_type + action_data 生成可读简述
    fn summarize_intent(
        action_type: &str,
        action_data: Option<&serde_json::Value>,
        location: &str,
    ) -> String {
        let data = action_data.cloned().unwrap_or(serde_json::Value::Null);

        match action_type {
            "说话" => {
                let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let target = data.get("target_agent_id").and_then(|v| v.as_str());
                match target {
                    Some(_) => format!("对某人说话：{}", content),
                    None => format!("向在场众人说话：{}", content),
                }
            }
            "私语" => {
                let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
                format!("向某人密语：{}", content)
            }
            "大喊" => {
                let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
                format!("大声喊道：{}", content)
            }
            "移动" => {
                let target = data
                    .get("target_location")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知地点");
                format!("从{}移动到{}", location, target)
            }
            "进食" => {
                let item = data
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("食物");
                format!("吃了{}", item)
            }
            "饮水" => {
                let item = data.get("item_id").and_then(|v| v.as_str()).unwrap_or("水");
                format!("喝了{}", item)
            }
            "采集" => {
                let resource = data
                    .get("target_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("资源");
                format!("采集{}", resource)
            }
            "拾取" => {
                let item = data
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("物品");
                format!("拾起{}", item)
            }
            "给予" => {
                let item = data
                    .get("item_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("物品");
                format!("给予{}", item)
            }
            "休息" => "原地休息".to_string(),
            other => format!("执行{}", other),
        }
    }

    /// 启动时主动从 Server 拉取 prompt_templates 并写盘
    ///
    /// 确保本地存在 prompt_templates.json 文件供下次冷启动使用。
    /// 失败不阻塞启动——WS ConfigUpdate 已在连接时更新了 runtime config。
    async fn fetch_prompt_templates_from_server(&self) {
        let Some(ref engine) = self.cognitive_engine else {
            return;
        };
        let Some(ref device_cfg) = self.device_config else {
            return;
        };

        let http_url = self.config.server.http_url.clone();
        let device_id = device_cfg.device_id;
        let auth_token = device_cfg.auth_token.clone();
        let engine = engine.clone();

        let client = reqwest::Client::new();
        let url = format!("{}/api/v1/agent/prompt-templates", http_url);
        let body = serde_json::json!({
            "device_id": device_id,
            "auth_token": auth_token,
        });

        match client.post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(data) => {
                        let hash = data["hash"].as_str().unwrap_or("");
                        let version = data["version"].as_str().unwrap_or("");
                        if let Some(content) = data.get("content") {
                            match cyber_jianghu_protocol::PromptTemplateConfig::from_json_value(
                                content.clone(),
                            ) {
                                Ok(config) => {
                                    info!(
                                        "启动拉取 prompt_templates 成功: version={}, hash={}",
                                        version,
                                        &hash[..12.min(hash.len())]
                                    );
                                    engine.update_prompt_template_from_config(config);
                                    engine.save_prompt_template_to_disk();
                                }
                                Err(e) => {
                                    warn!("启动拉取 prompt_templates 解析失败: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("启动拉取 prompt_templates 响应解析失败: {}", e);
                    }
                }
            }
            Ok(resp) => {
                warn!("启动拉取 prompt_templates 失败: status={}", resp.status());
            }
            Err(e) => {
                warn!("启动拉取 prompt_templates 请求失败: {}", e);
            }
        }
    }
}
