// ============================================================================
// 连接生命周期管理
// ============================================================================
//
// 处理 Agent 的连接、主循环和关闭
// 重连逻辑在 reconnect.rs 中
// ============================================================================

mod callbacks;
mod context;
mod death;
mod helpers;
mod reporting;
mod soul_cycle;
mod tick;

use anyhow::Result;
use cyber_jianghu_protocol::ServerMessage;
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::reconnect::{save_character_config_to_fs, should_log_retry};
use crate::component::social::RelationshipStore;
use crate::config::CharacterStatus;
use crate::infra::transport::ConnectError;

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

        self.setup_client_callbacks().await;

        // 等待注册确认（包含游戏规则）
        // Ok(None) = agent_id 为 nil，等待角色注册（保持连接，不 close/reconnect）
        let (agent_id, game_rules, world_building_rules, registered_name, is_alive) =
            match self.client.wait_for_registration().await {
                Ok(Some((id, rules, wb_rules, name, alive))) => (id, rules, wb_rules, name, alive),
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

        // 热更新认知引擎的动作列表缓存
        if let Some(ref engine) = self.cognitive_engine {
            engine.update_action_aliases(&game_rules.available_actions);
            // 注入 available_actions 供地魂 get_action_detail 工具使用
            engine.set_available_actions(game_rules.available_actions.clone());
        }

        // 注入 WorldStateStore 到 CognitiveEngine（供地魂 query_world 工具使用）
        if let (Some(engine), Some(store)) = (&self.cognitive_engine, &self.world_state_store) {
            engine.set_world_state_store(store.clone());
        }

        // 立即应用 world_building_rules 到 Validator（不等待后续 ConfigUpdate）
        if let (Some(validator), Some(wb_rules)) = (&self.validator, &world_building_rules) {
            let v = validator.clone();
            let rules = wb_rules.clone();
            v.update_rules(rules).await;
            info!(
                "已从 Registered 消息应用 world_building_rules v={} 到 Validator",
                wb_rules.version
            );
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

        self.build_and_set_server_message_callback().await;

        // 订阅死亡事件广播通道
        // 当 ServerMessage::AgentDied 到达时，callback 会写入 death_event_tx
        let mut death_rx = self
            .http_api_state
            .as_ref()
            .map(|s| s.death_event_tx.subscribe());

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
                                        *api_state.relationship_store.write().expect("rwlock poisoned") = Some(Arc::new(new_store));
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

                // 1.4 死亡事件（AgentDied 消息通过 broadcast channel 到达）
                // 独立于 WorldState 路径，解决死 agent 收不到 WorldState 的竞态问题
                death_msg = async {
                    if let Some(ref mut rx) = death_rx {
                        rx.recv().await.ok()
                    } else {
                        std::future::pending().await
                    }
                } => {
                    if let Some(cyber_jianghu_protocol::ServerMessage::AgentDied {
                        agent_id,
                        tick_id,
                        description,
                        ..
                    }) = death_msg
                        && !self.death_reported
                    {
                        self.handle_death(tick_id, agent_id, &description).await;
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

                    self.update_tick_state(&world_state).await;

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
                        self.handle_death(
                            world_state.tick_id,
                            world_state.agent_id.unwrap_or_default(),
                            death_desc,
                        ).await;
                        continue;
                    }

                    // 1.5b 已死亡 → 跳过决策循环（等待重生恢复）
                    if self.death_reported {
                        // 双源检查：优先 WS 回调值（动态），fallback 到 config 值（注册时下发的 game_rules）
                        let effective_delay_ticks = if self.rebirth_delay_ticks > 0 {
                            self.rebirth_delay_ticks
                        } else {
                            self.config.rebirth_delay_ticks()
                        };
                        if effective_delay_ticks > 0 {
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
                            // 无自动重生（WS 回调 + config 均为 0）：持续等待直到外部触发（通过 API 或重启）
                            continue;
                        }
                    }

                    // 构建记忆上下文（事件消费 + 遗忘 + 对话 + 交易提示 + triage）
                    let (mut memory_context, trade_hints) =
                        self.build_tick_memory_context(&world_state).await;

                    // 4.5 天魂执行叙事生成（上一轮行动结果，用于 memory_context 和 soul_cycle_record 回填）
                    {
                        let last_intents = last_intents_for_narrative.lock().expect("lock poisoned").clone();

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

                    // 三魂循环（ActorSoul → ReflectorSoul 审查 + 后置处理）
                    let soul_result = self
                        .run_three_soul_cycle(
                            &world_state,
                            &memory_context,
                            active_dream.as_deref(),
                            &last_intents_for_narrative,
                        )
                        .await?;
                    let mut final_intent = soul_result.intent;
                    let final_intent_validated = soul_result.validated;

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
                            // Pipeline 语义：失败只阻断后续 intent，前序成功 intent 已生效。
                            // 使用 mpsc channel 收集全部多 intent 结果
                            match self.client.wait_for_execution_result(self.config.llm.execution_result_timeout_ms).await {
                                Ok(results) if !results.is_empty() => {
                                    let success_count = results.iter().filter(|r| r.success).count();
                                    let total = results.len();
                                    let all_success = success_count == total;
                                    let first_failure = results.iter().find(|r| !r.success);

                                    debug!(
                                        "ExecutionResult: tick={}, {}/{} success",
                                        results[0].tick_id, success_count, total
                                    );

                                    // 建立 intent_id → (action_type, action_data) 映射表
                                    let intent_map: std::collections::HashMap<uuid::Uuid, (&cyber_jianghu_protocol::ActionType, &Option<serde_json::Value>)> = {
                                        let mut map = std::collections::HashMap::new();
                                        map.insert(final_intent.intent_id, (&final_intent.action_type, &final_intent.action_data));
                                        for si in &final_intent.subsequent_intents {
                                            map.insert(si.intent_id, (&si.action_type, &si.action_data));
                                        }
                                        map
                                    };

                                    // BUG-4b: intent 失败且 agent 已死亡 → 立即触发死亡处理
                                    if !self.death_reported && first_failure.is_some() {
                                        let is_dead_now = self.http_api_state.as_ref()
                                            .map(|s| s.is_dead.load(std::sync::atomic::Ordering::Relaxed))
                                            .unwrap_or(false);
                                        if is_dead_now {
                                            let reason_str = first_failure.and_then(|r| r.error.as_deref()).unwrap_or("");
                                            warn!(
                                                "Agent '{}' 检测到死亡（intent 失败后）: {}",
                                                self.character_name(), reason_str
                                            );
                                            self.death_reported = true;
                                            self.death_tick_id = Some(world_state.tick_id);

                                            if let Some(ref api_state) = self.http_api_state {
                                                self.rebirth_delay_ticks = api_state
                                                    .rebirth_delay_ticks.load(std::sync::atomic::Ordering::Relaxed);
                                            }

                                            if let Some(ref mut char_cfg) = self.character_config {
                                                char_cfg.status = crate::config::CharacterStatus::Dead;
                                                if let Some(ref api_state) = self.http_api_state {
                                                    let characters_dir = api_state.character_dir.read().await.clone();
                                                    if let Err(e) = save_character_config_to_fs(char_cfg, &characters_dir) {
                                                        warn!("Failed to persist death status: {}", e);
                                                    }
                                                }
                                            }

                                            death::maybe_schedule_auto_rebirth(
                                                self,
                                                world_state.agent_id.unwrap_or_default(),
                                                world_state.tick_id,
                                                "（intent失败路径）",
                                            ).await;
                                        }
                                    }

                                    // Outcome 写回：逐条记录每个 intent 的执行结果
                                    let context_hash = crate::component::memory::compute_context_hash(&world_state);
                                    for result in &results {
                                        let (action_type, action_data) = intent_map
                                            .get(&result.intent_id)
                                            .map(|(at, ad)| (at.to_string(), (*ad).clone()))
                                            .unwrap_or_else(|| ("unknown".to_string(), None));

                                        if let Some(ref engine) = self.cognitive_engine {
                                            engine.record_outcome(crate::component::memory::OutcomeRecord {
                                                action_type: action_type.clone(),
                                                action_data: action_data.clone(),
                                                result: if result.success {
                                                    crate::component::memory::OutcomeResult::Success
                                                } else {
                                                    crate::component::memory::OutcomeResult::Failed(
                                                        result.error.clone().unwrap_or_default()
                                                    )
                                                },
                                                context_hash: context_hash.clone(),
                                                tick_id: result.tick_id,
                                            });
                                        }
                                    }

                                    // Summary 更新
                                    if let Some(ref engine) = self.cognitive_engine {
                                        let label = if all_success {
                                            format!("成功: {}", final_intent.action_type)
                                        } else {
                                            let failed_action = first_failure
                                                .and_then(|r| intent_map.get(&r.intent_id))
                                                .map(|(at, _)| at.to_string())
                                                .unwrap_or_default();
                                            format!(
                                                "部分成功 ({}/{}): {} | 失败: {}",
                                                success_count, total,
                                                final_intent.action_type,
                                                failed_action
                                            )
                                        };
                                        engine.update_summary_outcome(label);
                                    }

                                    // 失败部分：注入失败原因到下轮推理上下文
                                    if let Some(failed) = first_failure {
                                        let reason = failed.error.clone().unwrap_or_default();
                                        let failed_action = intent_map
                                            .get(&failed.intent_id)
                                            .map(|(at, _)| at.to_string())
                                            .unwrap_or_default();
                                        {
                                            let mut guard = self.server_error_feedback.lock().await;
                                            *guard = Some(format!(
                                                "[pipeline 部分失败 ({}/{}): {} 失败原因: {}]",
                                                success_count, total, failed_action, reason
                                            ));
                                        }
                                    }

                                    // 更新执行结果到快照
                                    if let Some(ref api_state) = self.http_api_state {
                                        let mut snapshot = api_state.decision_context_snapshot.write().await;
                                        if let Some(s) = snapshot.as_mut() {
                                            let failed_action = first_failure
                                                .and_then(|r| intent_map.get(&r.intent_id))
                                                .map(|(at, _)| at.to_string());
                                            s.last_execution_result = Some(
                                                crate::infra::api::ExecutionSummary {
                                                    action_type: final_intent.action_type.to_string(),
                                                    success: all_success,
                                                    narrative: if all_success {
                                                        format!("{} intents all success", total)
                                                    } else {
                                                        format!(
                                                            "{}/{} success, {} 失败: {}",
                                                            success_count, total,
                                                            failed_action.as_deref().unwrap_or("unknown"),
                                                            first_failure.and_then(|r| r.error.clone()).unwrap_or_default()
                                                        )
                                                    },
                                                }
                                            );
                                        }
                                    }
                                }
                                Ok(_) => {
                                    debug!("ExecutionResult timeout ({}ms), no results received", self.config.llm.execution_result_timeout_ms);
                                }
                                Err(e) => {
                                    debug!("ExecutionResult poll error: {}", e);
                                }
                            }

                            if final_intent.action_type.as_str() != "休息" {
                                self.consecutive_idle_count = 0;
                                if let Some(ref container) = self.actor_llm_container {
                                    let llm = container.read().await;
                                    llm.reset_idle_count();
                                }
                            }
                            if final_intent.action_type.as_str() == "休息" {
                                self.maybe_rotate_model().await;
                            }

                            self.report_soul_cycle_and_compress(&final_intent).await;
                        }
                }
            }

            // 每个 tick 结束时持久化 token 统计
            crate::component::llm::token_tracking::persist_and_reset();
        }
    }

    /// 统一死亡处理：持久化状态 → 生成传记 → 调度重生
    ///
    /// 可从两条路径调用：
    /// 1. WorldState.events_log 中包含 DeathNotification（死亡后最后一条 WorldState 恰好到达）
    /// 2. AgentDied 消息通过 death_event_tx 广播到达
    async fn handle_death(
        &mut self,
        death_tick_id: i64,
        dead_agent_id: Uuid,
        death_description: &str,
    ) {
        warn!(
            "Agent '{}' has died (tick={}): {}",
            self.character_name(),
            death_tick_id,
            death_description
        );
        self.death_reported = true;
        self.death_tick_id = Some(death_tick_id);

        // 从 HttpApiState 同步死亡标记（AgentDied 回调可能已经设置，确保一致性）
        if let Some(ref api_state) = self.http_api_state {
            api_state
                .is_dead
                .store(true, std::sync::atomic::Ordering::Relaxed);
            self.rebirth_delay_ticks = api_state
                .rebirth_delay_ticks
                .load(std::sync::atomic::Ordering::Relaxed);
        }

        // 持久化死亡状态到 character.yaml
        if let Some(ref mut char_cfg) = self.character_config {
            char_cfg.status = CharacterStatus::Dead;
            if let Some(ref api_state) = self.http_api_state {
                let characters_dir = api_state.character_dir.read().await.clone();
                if let Err(e) = save_character_config_to_fs(char_cfg, &characters_dir) {
                    warn!("Failed to persist death status: {}", e);
                }
            }
        }

        // 死亡时触发传记生成（fire-and-forget，不阻塞重生调度）
        if let Some(ref api_state) = self.http_api_state {
            let state = api_state.clone();
            let bio_agent_id = dead_agent_id;
            tokio::spawn(async move {
                info!("[biography] 死亡触发传记生成: agent={}", bio_agent_id);
                const MAX_RETRIES: u32 = 3;
                const RETRY_DELAY_SECS: u64 = 30;
                for attempt in 0..MAX_RETRIES {
                    match crate::infra::api::handlers::generate_biography_for_agent(
                        &state,
                        bio_agent_id,
                    )
                    .await
                    {
                        Ok(bio) => {
                            info!("[biography] 死亡传记生成成功: {}字", bio.chars().count());
                            return;
                        }
                        Err(e) => {
                            warn!(
                                "[biography] 死亡传记生成失败 (attempt {}/{}): {}",
                                attempt + 1,
                                MAX_RETRIES,
                                e
                            );
                            if attempt + 1 < MAX_RETRIES {
                                tokio::time::sleep(std::time::Duration::from_secs(
                                    RETRY_DELAY_SECS,
                                ))
                                .await;
                            }
                        }
                    }
                }
                warn!("[biography] 死亡传记生成最终失败: agent={}", bio_agent_id);
            });
        }

        // 调度自动重生
        death::maybe_schedule_auto_rebirth(self, dead_agent_id, death_tick_id, "").await;
    }
}
