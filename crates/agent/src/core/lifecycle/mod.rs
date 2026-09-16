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
mod soul_cycle_support;
mod tick;
mod world_tick;

use anyhow::Result;
use cyber_jianghu_protocol::{ServerMessage, WorldEvent};
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::reconnect::{save_character_config_to_fs, should_log_retry};
use crate::component::social::RelationshipStore;
use crate::config::CharacterStatus;
use crate::infra::transport::ConnectError;

/// select! WorldState 臂的处理结果：Continue = 主循环进入下一轮
#[derive(PartialEq, Eq)]
enum ArmOutcome {
    Continue,
    Handled,
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
                    "Agent '{}' status is {:?}, waiting for rebirth",
                    character.name, character.status
                );
            } else {
                warn!("No active character, waiting for character creation");
            }
            // 缺口修复 2026-09-15：此路径原先直接等待，容器重建会丢失原进程
            // 调度的转世定时器（agent-4 事故），且全新安装无任何兑底。
            // 现区分两种情况：
            //   a) 存在可自动转世的死亡角色 → 重新调度转世（与注册返回 nil 路径对齐）
            //   b) 无可转世角色（全新安装/全部归隐）→ 布防自动注册倒计时
            //      （runtime.auto_register_timeout_secs，默认 30 分钟；面板引导
            //      注册并显示倒计时，超时自动生成角色）
            if !death::try_schedule_startup_rebirth(self).await {
                self.arm_auto_register_if_absent().await;
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

        // 训练 trace 回传 sender 注入（连接成功后 intent_sender 才可用）
        // 代理1校准：init_trace_recorder 在 main 顶端调用（连接前），sender 此处才注入
        if let Some(sender) = self.client.intent_sender().await {
            crate::infra::api::trace::set_upload_sender(sender);
        }

        // 等待注册确认（包含游戏规则）
        // Ok(None) = agent_id 为 nil，等待角色注册（保持连接，不 close/reconnect）
        let (
            agent_id,
            game_rules,
            world_building_rules,
            registered_name,
            is_alive,
            narrative_config,
            narrative_config_hash,
        ) = match self.client.wait_for_registration().await {
            Ok(Some((id, rules, wb_rules, name, alive, nc, nc_hash))) => {
                (id, rules, wb_rules, name, alive, nc, nc_hash)
            }
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
                // 注册返回 nil：设备无存活角色（容器重启/断连期间死亡后重连的常见入口）。
                // auto_rebirth 开启且本地角色为 Dead 时自动转世，
                // 否则将永久停在等待转生模式（无面板干预时无人唤醒）。
                if let Some(ref char_cfg) = self.character_config
                    && char_cfg.status == crate::config::CharacterStatus::Dead
                    && let Some(old_id) = char_cfg.agent_id.filter(|id| !id.is_nil())
                {
                    death::maybe_schedule_auto_rebirth(
                        self,
                        old_id,
                        0,
                        "（注册时角色已死亡，自动转世）",
                    )
                    .await;
                } else {
                    // 无可自动转世角色（全新安装/全部归隐）→ 布防自动注册兑底
                    self.arm_auto_register_if_absent().await;
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
                if let Err(e) = api_state.death_event_tx.send(death_msg) {
                    tracing::warn!(
                        "death_event_tx.send（lifecycle）失败（receiver 可能已 drop）：{e:?}"
                    );
                }
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
                if let Err(e) = api_state.death_event_tx.send(death_msg) {
                    tracing::warn!(
                        "death_event_tx.send（lifecycle site 2）失败（receiver 可能已 drop）：{e:?}"
                    );
                }
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

            // 同类缺口：此路径原先也不调度转世/兑底，断连期间死亡后直等外部干预。
            // 死亡角色可转世则调度，否则布防自动注册。
            if !death::try_schedule_startup_rebirth(self).await {
                self.arm_auto_register_if_absent().await;
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
            engine.update_action_index(&game_rules.available_actions);
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

        // 注入从 Registered WS 消息获取的 narrative_config
        if let Some(ref nc) = narrative_config
            && let Some(ref api_state) = self.http_api_state
        {
            *api_state.narrative_config.write().await = Some(nc.clone());
            let hash = narrative_config_hash.as_deref();
            if let Err(e) = crate::config::save_narrative_config_to_disk(nc, hash) {
                warn!("保存 narrative_config 到磁盘失败: {}", e);
            } else {
                info!("已从 Registered 消息注入 narrative_config");
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
                                    let new_system_prompt = if let Some(ref api_state) = self.http_api_state {
                                        api_state.pending_rebirth_system_prompt.write().await.take()
                                    } else {
                                        None
                                    };

                                    if let Some(new_id) = new_agent_id {
                                        // 更新 HttpApiState.agent_id
                                        if let Some(ref api_state) = self.http_api_state {
                                            *api_state.agent_id.write().await = new_id;
                                        }

                                        // 更新本地 character_config：复用旧角色信息，仅换 agent_id + 状态
                                        if let Some(ref mut char_cfg) = self.character_config {
                                            char_cfg.agent_id = Some(new_id);
                                            char_cfg.status = crate::config::CharacterStatus::Alive;
                                            if let Some(system_prompt) = new_system_prompt.clone() {
                                                char_cfg.system_prompt = Some(system_prompt);
                                            }
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

                                            // 跨代记忆 fix (#51)：重建 MemoryManager（新 agent_id → 新 DB）
                                            // 重生后是新的人，前世的情景/语义/工作记忆应清空。
                                            // 技能（skills）通过 server 推送，不受影响。
                                            if let Some(template) = &api_state.memory_config_template {
                                                let mut mem_config = template.clone();
                                                mem_config.agent_id = new_id;
                                                mem_config.db_dir = api_state.data_dir.clone();
                                                match crate::component::memory::MemoryManager::new(mem_config) {
                                                    Ok(new_manager) => {
                                                        let new_mem_arc = std::sync::Arc::new(
                                                            tokio::sync::RwLock::new(new_manager),
                                                        );
                                                        self.memory_manager = Some(new_mem_arc.clone());
                                                        if let Some(ref engine) = self.cognitive_engine {
                                                            engine.set_memory_manager(new_mem_arc.clone());
                                                        }
                                                        *api_state.memory_manager.write().await =
                                                            Some(new_mem_arc);
                                                        info!(
                                                            "转世重生: MemoryManager 已重初始化 (new_id={})，前世记忆已清空",
                                                            new_id
                                                        );
                                                    }
                                                    Err(e) => {
                                                        warn!("转世重生: MemoryManager 重初始化失败: {}", e);
                                                    }
                                                }
                                            }

                                            // 跨代记忆 fix (#51)：重建 PersonaStore（新 agent_id → 新 DB）
                                            // 人格/经验值/动态特质也应清空，新生角色从默认人设开始。
                                            let new_persona_path = api_state
                                                .data_dir
                                                .join(format!("persona_{}.db", new_id));
                                            let persona_config =
                                                crate::component::persona::PersonaPersistenceConfig::default();
                                            match crate::component::persona::PersonaStore::open(
                                                new_id,
                                                &new_persona_path,
                                                persona_config,
                                            ) {
                                                Ok(new_store) => {
                                                    self.persona_store = Some(std::sync::Arc::new(new_store));
                                                    info!(
                                                        "转世重生: PersonaStore 已重初始化 (new_id={})，前世人格已清空",
                                                        new_id
                                                    );
                                                }
                                                Err(e) => {
                                                    warn!("转世重生: PersonaStore 重初始化失败: {}", e);
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
                                if self
                                    .handle_world_state_tick(
                                        agent_id,
                                        world_state,
                                        &mut death_rx,
                                        &last_intents_for_narrative,
                                    )
                                    .await?
                                        == ArmOutcome::Continue
                                {
                                    continue;
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
    /// 1. WorldState.events_log 中包含「自身」的 DeathNotification（比对死者 ID，
    ///    见 death::find_self_death；目击他人死亡不触发本函数）
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

        if let Some(ref store) = self.persona_store
            && store.config_flush_on_death()
            && let Err(e) = self.persona.read(|p| store.snapshot_now(p, death_tick_id))
        {
            warn!("persona 死亡 flush 失败: {}", e);
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
            crate::infra::api::handlers::spawn_biography_generation_with_retry(
                api_state,
                dead_agent_id,
                "死亡",
            );
        }

        // 调度自动重生
        death::maybe_schedule_auto_rebirth(self, dead_agent_id, death_tick_id, "").await;
    }
}

/// 合并事件流：当前快照的 events_log 在前，队列找回的事件去重后追加。
///
/// 去重键 = 事件整体 JSON 序列化：同一事件可能同时出现在最新快照与队列中
/// （例如携带死亡事件的快照未被覆盖时）；同名不同 metadata 的事件不去重。
fn merge_events_log(base: Vec<WorldEvent>, pending: Vec<WorldEvent>) -> Vec<WorldEvent> {
    let mut seen: std::collections::HashSet<String> = base
        .iter()
        .filter_map(|e| serde_json::to_string(e).ok())
        .collect();
    let mut merged = base;
    for event in pending {
        let key = serde_json::to_string(&event).unwrap_or_default();
        if !key.is_empty() && seen.insert(key) {
            merged.push(event);
        }
    }
    merged
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
