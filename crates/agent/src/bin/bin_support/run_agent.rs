//! run_agent 主流程（Cognitive/Claw 两模式装配与事件循环）

use super::*;

pub(crate) async fn run_agent(port: u16, mode: String, server: Option<String>) -> Result<()> {
    let mut config = load_config()?.ok_or_else(|| {
        anyhow::anyhow!("配置文件不存在且无法从环境变量构造（character_generation 为必填项）")
    })?;

    // Fail Fast: 校验 EarthSoul 配置
    config
        .earth_soul
        .validate()
        .context("earth_soul 配置校验失败")?;

    // Ensure servers_dir is set (#[serde(default)] means it's empty after from_file)
    if config.servers_dir.as_os_str().is_empty() {
        config.servers_dir = cyber_jianghu_agent::config::data_base_dir().join("servers");
    }

    let runtime_mode = match mode.to_lowercase().as_str() {
        "cognitive" => {
            info!("使用 Cognitive 模式（内置 LLM 决策）");
            RuntimeMode::Cognitive
        }
        "claw" => {
            info!("使用 Claw 模式（等待外部调度器）");
            RuntimeMode::Claw
        }
        _ => {
            info!("未知模式 '{}'，使用 Cognitive 模式", mode);
            RuntimeMode::Cognitive
        }
    };

    // Determine server URL (CLI arg overrides config)
    let ws_url = server.as_deref().unwrap_or(&config.server.ws_url);
    info!("连接服务器: {}", ws_url);

    // Set config path for hot reload
    let config_path = config_path();
    let mut config_for_builder = config.clone();
    config_for_builder.config_path = config_path.clone();
    config_for_builder.runtime.mode = runtime_mode;

    // Ensure device identity
    let device = ensure_device(&config, ws_url).await?;
    info!("Device ID: {}", device.device_id);

    // Select character from filesystem
    let server_dir = config.server_dir(ws_url);
    let initial_character = select_character(&server_dir);

    // Determine the runtime agent_id:
    // - If an alive character with a valid agent_id exists, use it (so the web panel
    //   correctly marks is_current in list_characters_handler).
    // - Otherwise fall back to the device UUID (agent not registered yet).
    let runtime_agent_id = if let Some(ref character) = initial_character
        && let Some(agent_uuid) = character.agent_id
    {
        info!("使用已有角色 UUID 作为运行时 agent_id: {}", agent_uuid);
        agent_uuid
    } else {
        device.device_id
    };

    // Arc-wrapped so HTTP handlers can read the current agent_id at any time.
    // This IS the state.agent_id that list_characters_handler compares against.
    let runtime_agent_id = Arc::new(RwLock::new(runtime_agent_id));
    let (reconnect_tx, _reconnect_rx) =
        tokio::sync::broadcast::channel::<cyber_jianghu_agent::infra::api::ReconnectRequest>(64);

    // Early HTTP API startup based on mode
    let _early_api_state: Option<Arc<cyber_jianghu_agent::infra::api::HttpApiState>>;
    let _early_claw_setup: Option<LateClawSetup>;
    let early_actual_port: u16;

    match runtime_mode {
        RuntimeMode::Cognitive => {
            let (api_state, actual_port) = start_http_api_server(
                port,
                runtime_agent_id.clone(),
                &config,
                ws_url,
                &device,
                server_dir.clone(),
                Some(reconnect_tx.clone()),
            )
            .await?;
            info!("HTTP API 已启动: http://localhost:{}", actual_port);
            info!("Web 面板: http://localhost:{}/", actual_port);
            info!("角色管理: http://localhost:{}/index.html", actual_port);
            _early_api_state = Some(api_state);
            _early_claw_setup = None;
            early_actual_port = actual_port;
        }
        RuntimeMode::Claw => {
            let setup = start_claw_server(
                port,
                runtime_agent_id.clone(),
                &config,
                ws_url,
                &device,
                server_dir.clone(),
            )?;
            _early_api_state = Some(setup.api_state.clone());
            _early_claw_setup = Some(LateClawSetup {
                shared_state: setup.shared_state.clone(),
                api_state: setup.api_state.clone(),
                server_msg_tx: setup.server_msg_tx.clone(),
            });
            early_actual_port = setup.actual_port;
            info!(
                "Claw HTTP API 已启动: http://localhost:{}",
                early_actual_port
            );
        }
    }

    // 自更新后台任务（硬禁用/配置关闭/容器内时内部自行退出并记录原因）
    if let Some(api_state) = _early_api_state.as_ref() {
        tokio::spawn(cyber_jianghu_agent::infra::updater::run_background(
            api_state.updater.clone(),
        ));
    }

    // Now check if we need to wait for character creation
    let character = match initial_character {
        Some(c) if c.agent_id.is_some() && c.status == CharacterStatus::Alive => c,
        _ => {
            // 角色未就绪 — 先注入 LLM container 以支持角色创建时的 LLM 调用
            if runtime_mode == RuntimeMode::Cognitive
                && let Some(ref early_state) = _early_api_state
            {
                let llm = create_llm_client(runtime_mode, &config, None)?;
                let container: std::sync::Arc<
                    tokio::sync::RwLock<
                        std::sync::Arc<dyn cyber_jianghu_agent::component::llm::LlmClient>,
                    >,
                > = std::sync::Arc::new(tokio::sync::RwLock::new(llm.clone()));
                *early_state.llm_container.write().await = Some(container);
                info!("LLM container 已预注入 HttpApiState（角色创建前）");
            }
            info!("尚未创建角色，等待角色创建...");
            info!(
                "请通过 Web 面板创建角色: http://localhost:{}/index.html",
                early_actual_port
            );
            await_character_loop(&server_dir, &config, _early_api_state.as_ref()).await?;
            // After waiting, character MUST exist
            select_character(&server_dir).context("Character not found after waiting")?
        }
    };

    let data_dir = server_dir
        .join("characters")
        .join(
            character
                .agent_id
                .expect("character must have agent_id")
                .to_string(),
        )
        .join("data");

    let persona_info = Some(cyber_jianghu_agent::soul::reflector::PersonaInfo {
        name: Some(character.name.clone()),
        gender: character.gender.clone(),
        age: character.age,
        personality: character.personality.clone(),
        values: character.values.clone(),
    });

    // 根据模式创建决策回调和相关组件
    let maybe_callback_setup: Option<CallbackSetup>;
    let cognitive_death_event_tx: Option<tokio::sync::broadcast::Sender<ServerMessage>>;

    // ========================================================================
    // 阶段 1: 按模式创建 LLM 客户端 — 这是两模式唯一差异点
    // ========================================================================
    #[allow(clippy::type_complexity)]
    let (llm_client, llm_container, api_state): (
        Arc<dyn cyber_jianghu_agent::component::llm::LlmClient>,
        Arc<tokio::sync::RwLock<Arc<dyn cyber_jianghu_agent::component::llm::LlmClient>>>,
        Arc<cyber_jianghu_agent::infra::api::HttpApiState>,
    ) = match runtime_mode {
        RuntimeMode::Cognitive => {
            let llm = create_llm_client(runtime_mode, &config, None)?;
            let llm_arc: Arc<dyn cyber_jianghu_agent::component::llm::LlmClient> = llm.clone();
            let container = Arc::new(RwLock::new(llm_arc.clone()));

            let early = _early_api_state
                .as_ref()
                .expect("early api_state must exist");
            let state = early.clone();

            // 浏览器打开 Web 面板
            let browser_url = format!("http://localhost:{}/", early_actual_port);
            let is_container = std::path::Path::new("/app/.dockerenv").exists();
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                if is_container {
                    debug!("浏览器请手动打开: {}", browser_url);
                } else {
                    match open::that_detached(&browser_url) {
                        Ok(_) => info!("浏览器已打开: {}", browser_url),
                        Err(e) => debug!("无法自动打开浏览器: {}，请手动访问: {}", e, browser_url),
                    }
                }
            });

            maybe_callback_setup = Some(CallbackSetup {
                shared_state: None,
                api_state: state.clone(),
                server_msg_tx: None,
                runtime_agent_id: runtime_agent_id.clone(),
                persona_info: persona_info.clone(),
            });
            cognitive_death_event_tx = Some(state.death_event_tx.clone());

            (llm_arc, container, state)
        }
        RuntimeMode::Claw => {
            let setup = _early_claw_setup.expect("early claw setup must exist");
            let llm_response_rx = setup
                .shared_state
                .llm_response_rx
                .lock()
                .expect("lock poisoned")
                .take();

            let openclaw_bridge = Arc::new(OpenClawBridge::new(
                setup.shared_state.upstream_tx.clone(),
                BridgeConfig::default(),
            ));
            let llm: Arc<dyn cyber_jianghu_agent::component::llm::LlmClient> =
                openclaw_bridge.clone();
            let container = Arc::new(RwLock::new(llm.clone()));

            // LLM 响应转发任务（Claw 独有）
            if let Some(mut response_rx) = llm_response_rx {
                let bridge = openclaw_bridge.clone();
                tokio::spawn(async move {
                    while let Some((request_id, result)) = response_rx.recv().await {
                        bridge.handle_response(
                            &request_id,
                            result.map_err(|e| anyhow::anyhow!("{}", e)),
                        );
                    }
                    info!("LLM 响应转发任务结束");
                });
                info!("LLM 响应转发任务已启动");
            }

            maybe_callback_setup = Some(CallbackSetup {
                shared_state: Some(setup.shared_state.clone()),
                api_state: setup.api_state.clone(),
                server_msg_tx: Some(setup.server_msg_tx.clone()),
                runtime_agent_id: runtime_agent_id.clone(),
                persona_info: persona_info.clone(),
            });
            cognitive_death_event_tx = None;

            (llm, container, setup.api_state.clone())
        }
    };

    // ========================================================================
    // 阶段 2: 统一初始化 — 两模式共享的大脑
    // ========================================================================
    let agent_name = character.name.as_str();
    let agent_id = device.device_id;
    let persona_description = character.generate_system_prompt();

    let initial_persona = cyber_jianghu_agent::component::persona::DynamicPersona::new(
        agent_id,
        agent_name,
        &persona_description,
    );

    let persona_persistence_config = cyber_jianghu_agent::config::load_persona_persistence_config(
        &cyber_jianghu_agent::config::config_dir(),
    );
    let persona_agent_id = character.agent_id.unwrap_or_else(Uuid::new_v4);
    let persona_db_path = data_dir.join(format!("persona_{}.db", persona_agent_id));
    let persona_store = match cyber_jianghu_agent::component::persona::PersonaStore::open(
        persona_agent_id,
        &persona_db_path,
        persona_persistence_config,
    ) {
        Ok(store) => {
            info!("PersonaStore 已初始化");
            Some(std::sync::Arc::new(store))
        }
        Err(e) => {
            warn!("PersonaStore 初始化失败: {}，继续无持久化", e);
            None
        }
    };
    let resolved_initial_persona = if let Some(ref store) = persona_store {
        match store.load_or_default(initial_persona.clone()) {
            Ok(p) => p,
            Err(e) => {
                warn!("persona 加载失败，使用默认初始值: {}", e);
                initial_persona
            }
        }
    } else {
        initial_persona
    };
    let persona =
        cyber_jianghu_agent::component::persona::ThreadSafePersona::new(resolved_initial_persona);
    // 接入 HTTP API：state_stream 视图(主角名/情绪)、认知上下文、叙事更新均消费 persona；
    // 此前该字段恒 None，四处消费者全部静默跳过（死接线）
    api_state.set_dynamic_persona(persona.clone());

    let cognitive_config = CognitiveEngineConfig {
        agent_name: agent_name.to_string(),
        temperature: config.llm.temperature,
        max_tokens_per_stage: config.llm.max_tokens,
    };
    let mut engine = CognitiveEngine::new(llm_client.clone(), cognitive_config, &persona);

    // Outcome Memory（Hermes 模式）
    let outcome_db_path = data_dir.join("outcome_memory.db");
    let outcome_prompt_limit = config.memory.outcome_prompt_limit;
    let outcome_max_records = config.memory.outcome_max_records;
    match cyber_jianghu_agent::component::memory::OutcomeMemory::with_max_records(
        &outcome_db_path,
        outcome_prompt_limit,
        outcome_max_records,
    ) {
        Ok(mem) => {
            info!(
                "Outcome memory initialized at {}",
                outcome_db_path.display()
            );
            engine.set_outcome_memory(mem);
        }
        Err(e) => {
            warn!(
                "Failed to initialize outcome memory: {}. Running without it.",
                e
            );
        }
    }

    // Conversation History（长窗口对话）
    let conv_db_path = data_dir.join("conversation_history.db");
    match cyber_jianghu_agent::component::llm::conversation::ConversationHistory::new(
        &conv_db_path,
        &persona_description,
        config.llm.context_window_tokens as usize,
        config.llm.keep_recent_turns as usize,
        config.llm.summary_trigger_ratio,
    ) {
        Ok(history) => {
            info!(
                "Conversation history initialized at {} (max_tokens={}, keep_recent={})",
                conv_db_path.display(),
                config.llm.context_window_tokens,
                config.llm.keep_recent_turns,
            );
            engine.set_conversation_history(history);
        }
        Err(e) => {
            warn!(
                "Failed to initialize conversation history: {}. Running without it.",
                e
            );
        }
    }

    // 设置 NarrativeSummaryWindow 窗口大小
    engine.set_narrative_window_size(config.llm.narrative_window_size);

    // 设置流式 LLM
    engine.set_enable_streaming(config.llm.enable_streaming);

    let cognitive_engine = Arc::new(engine);

    // 决策回调
    let decision_with_chain: DecisionWithChainCallback = Arc::new(cognitive_decision_with_chain(
        cognitive_engine.clone(),
        CognitiveDecisionConfig::default().max_retries,
    ));

    let cognitive_engine_for_memory = cognitive_engine.clone();
    let cognitive_engine_for_decision = cognitive_engine.clone();
    let decision: DecisionCallback = Arc::new(move |tick_id: i64, agent_id: Uuid| {
        let engine = cognitive_engine_for_decision.clone();
        Box::pin(async move {
            match engine.think(tick_id, agent_id).await {
                Ok(chain) => chain.final_intent,
                Err(e) => {
                    error!("[cognitive] Decision failed: {}", e);
                    Intent::new(agent_id, tick_id, "休整", None)
                        .with_thought(format!("认知失败: {}", e))
                }
            }
        })
    });

    let decision_with_memory: cyber_jianghu_agent::runtime::DecisionWithMemoryCallback =
        Arc::new(move |tick_id: i64, agent_id: Uuid, memory_context: &str| {
            let engine = cognitive_engine_for_memory.clone();
            let memory_context = memory_context.to_string();
            Box::pin(async move {
                match engine
                    .think_with_memory(tick_id, agent_id, &memory_context)
                    .await
                {
                    Ok(chain) => chain.final_intent,
                    Err(e) => {
                        error!("[cognitive] Decision with memory failed: {}", e);
                        Intent::new(agent_id, tick_id, "休整", None)
                            .with_thought(format!("认知失败: {}", e))
                    }
                }
            })
        });

    // RelationshipStore（per-character DB，与 HTTP API 路径对齐）
    let agent_id_for_rel = character.agent_id.unwrap_or_else(Uuid::new_v4);
    let relationship_db_path = data_dir.join(format!("relationships_{}.db", agent_id_for_rel));
    let relationship_store = match cyber_jianghu_agent::component::social::RelationshipStore::open(
        agent_id_for_rel,
        &relationship_db_path,
    ) {
        Ok(store) => {
            info!("RelationshipStore 已初始化");
            Some(store)
        }
        Err(e) => {
            tracing::warn!("RelationshipStore 初始化失败: {}，继续无关系存储", e);
            None
        }
    };

    // AgentBuilder
    let reconnect_rx = api_state
        .reconnect_tx
        .as_ref()
        .map(|tx| tx.subscribe())
        .expect("reconnect_tx must be initialized");

    let mut builder = AgentBuilder::new(config_for_builder, decision)
        .device_config(device.clone())
        .data_dir(data_dir.clone())
        .with_decision_chain(decision_with_chain)
        .with_decision_memory(decision_with_memory)
        .with_llm_container(llm_container.clone())
        .with_llm_client(
            llm_client.clone(),
            Some(WorldBuildingRules {
                version: String::new(),
                era: EraSettings {
                    name: String::new(),
                    tech_level: String::new(),
                    social_structure: String::new(),
                },
                allowed_concepts: Vec::new(),
                forbidden_concepts: Vec::new(),
                narrative_rules: String::new(),
                last_updated: String::new(),
                rules_json: None,
                known_item_ids: Vec::new(),
            }),
        )
        .with_http_api_state(api_state.clone())
        .with_reconnect_rx(reconnect_rx)
        .cognitive_engine(cognitive_engine.clone());

    // ChaosGenerator
    builder = builder.with_chaos_generator(cyber_jianghu_agent::soul::actor::ChaosGenerator::new(
        cyber_jianghu_agent::soul::actor::ChaosConfig::default(),
    ));

    if let Some(store) = relationship_store {
        builder = builder.with_relationship_store(store);
    }

    if let Some(store) = persona_store.clone() {
        builder = builder.with_persona_store(store);
    }

    builder = builder.character_config(character.clone());

    // ImmediateHandler（即时事件处理：SQLite 持久化 + Session Triage LLM）
    {
        builder = builder.with_immediate_handler();
        info!("即时事件处理器已创建");
    }

    // DeltaEngine + AttentionController（Token 优化模式）
    let token_opt_enabled = config.token_optimization.enabled;
    let world_state_store =
        std::sync::Arc::new(cyber_jianghu_agent::component::state_store::WorldStateStore::new());

    if token_opt_enabled {
        let delta_config = cyber_jianghu_agent::component::delta_engine::DeltaConfig {
            change_percentage_threshold: config
                .token_optimization
                .delta
                .change_percentage_threshold,
            survival_critical_urgency_threshold: config
                .token_optimization
                .delta
                .survival_critical_urgency_threshold,
            // Registered 下发 narrative_config 后由 lifecycle 注入实际显示名；
            // 此处空表使静态兜底先生效
            attribute_display_names: Default::default(),
        };
        let attention_config = config.token_optimization.attention.clone();
        builder = builder
            .with_world_state_store(world_state_store.clone())
            .with_delta_engine(
                cyber_jianghu_agent::component::delta_engine::DeltaEngine::new(delta_config),
            )
            .with_attention_controller(
                cyber_jianghu_agent::component::attention::AttentionController::new(
                    attention_config,
                ),
            );
        info!("DeltaEngine + AttentionController 已初始化（Token 优化模式）");
    }

    // Emotion 系统配置加载
    {
        let emotion_path = config
            .server_dir(&config.server.ws_url)
            .join("emotion.yaml");
        if emotion_path.exists() {
            match std::fs::read_to_string(&emotion_path) {
                Ok(content) => match serde_yaml::from_str::<
                    cyber_jianghu_agent::component::emotion::config::EmotionConfig,
                >(&content)
                {
                    Ok(emotion_config) => {
                        builder = builder.with_emotion_config(emotion_config);
                        info!("情绪系统配置已加载: {}", emotion_path.display());
                    }
                    Err(e) => warn!("emotion.yaml 解析失败，情绪系统禁用: {}", e),
                },
                Err(e) => warn!("emotion.yaml 读取失败，情绪系统禁用: {}", e),
            }
        } else {
            info!("emotion.yaml 不存在，情绪系统禁用");
        }
    }

    let mut agent = builder.build();

    // 注入 world_state_store 到 HttpApiState（供 Claw 模式 Delta Engine 使用）
    if token_opt_enabled {
        *api_state
            .world_state_store
            .write()
            .expect("rwlock poisoned") = Some(world_state_store.clone());
        info!("world_state_store 已注入 HttpApiState");
    }

    // 注入 relationship_store 到 HttpApiState
    if let Some(store) = agent.relationship_store() {
        *api_state
            .relationship_store
            .write()
            .expect("rwlock poisoned") = Some(Arc::new(store.clone()));
        info!("relationship_store 已注入 HttpApiState");
    }

    // 注入 LLM container 到 HttpApiState（支持热重载重建）
    {
        *api_state.llm_container.write().await = Some(llm_container.clone());
        info!("LLM container 已注入 HttpApiState（支持热重载）");
    }

    // 注入 MemoryManager 到 HttpApiState（与 Agent 共享同一实例）
    if let Some(mm) = agent.memory_manager() {
        // mm is Arc<tokio::sync::RwLock<MemoryManager>>
        // Clone the Arc to share with HttpApiState
        let mm = Arc::clone(mm);
        *api_state.memory_manager.write().await = Some(mm);
        info!("MemoryManager 已注入 HttpApiState（与 Agent 共享）");
    } else {
        info!("Agent 未创建 MemoryManager");
    }

    // 死亡事件回调：Cognitive 模式通过 lifecycle 处理死亡标记
    if let Some(death_tx) = cognitive_death_event_tx {
        let death_tx_clone = death_tx.clone();
        let api_state_clone = api_state.clone();
        agent
            .set_server_msg_callback(std::sync::Arc::new(move |msg: ServerMessage| {
                if let ServerMessage::AgentDied {
                    rebirth_delay_ticks,
                    ..
                } = &msg
                {
                    api_state_clone
                        .is_dead
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    api_state_clone
                        .rebirth_delay_ticks
                        .store(*rebirth_delay_ticks, std::sync::atomic::Ordering::Relaxed);
                    if let Err(e) = death_tx_clone.send(msg) {
                        tracing::warn!("death_tx.send 失败（receiver 可能已 drop）：{e:?}");
                    }
                }
            }))
            .await;
    }

    // 注册回调 + Claw 模式 downstream 转发
    if let Some(setup) = maybe_callback_setup {
        let shared_state_clone = setup.shared_state.clone();
        let api_state_clone = setup.api_state.clone();
        let runtime_agent_id_clone = setup.runtime_agent_id.clone();
        let persona_clone = setup.persona_info.clone();

        agent.set_registration_callback(std::sync::Arc::new(move |server_agent_id: Uuid| {
            // block_in_place 允许在 multi-threaded runtime 的同步闭包中执行 async 操作
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let old_id = *runtime_agent_id_clone.read().await;
                    info!("更新 runtime agent_id: {} -> {}", old_id, server_agent_id);
                    *runtime_agent_id_clone.write().await = server_agent_id;

                    // WsSharedState 注入 — 仅 Claw 模式有值
                    if let Some(ref shared_state) = shared_state_clone {
                        if let Some(ref validator) = api_state_clone.intent_validator {
                            let mut validator_guard = shared_state.intent_validator.write().await;
                            *validator_guard = Some(validator.clone());
                            info!("Validator injected into WsSharedState");
                        }
                        {
                            let game_rules = api_state_clone.game_rules.read().await.clone();
                            let mut guard = shared_state.game_rules.write().await;
                            *guard = game_rules;
                        }

                        if let Some(ref persona) = persona_clone {
                            let mut persona_guard = shared_state.persona.write().await;
                            *persona_guard = Some(persona.clone());
                            info!("Persona injected into WsSharedState");
                        }
                    }
                });
            });
        }));

        // server_msg_callback: Claw 模式做 downstream 转发
        if let Some(ref server_msg_tx) = setup.server_msg_tx {
            let tx_clone = server_msg_tx.clone();
            agent
                .set_server_msg_callback(std::sync::Arc::new(move |msg: ServerMessage| {
                    let current_tick = 0;
                    if let Some(downstream) =
                        DownstreamMessage::from_server_message(msg, current_tick)
                        && let Err(e) = tx_clone.send(downstream)
                    {
                        tracing::warn!("downstream tx.send 失败（receiver 可能已 drop）：{e:?}");
                    }
                }))
                .await;
        }
    }

    // 外层循环：run() 返回 Ok(()) 表示需要重启（等待转生后重新连接）
    // Err 才是真正的致命错误
    // 支持 SIGTERM / Ctrl+C 优雅关闭
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("收到 Ctrl+C 信号");
        if let Err(e) = shutdown_tx_clone.send(()).await {
            tracing::warn!("shutdown_tx.send（Ctrl+C）失败（receiver 可能已 drop）：{e:?}");
        }
    });

    #[cfg(unix)]
    {
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
            sigterm.recv().await;
            info!("收到 SIGTERM 信号");
            if let Err(e) = shutdown_tx_clone.send(()).await {
                tracing::warn!("shutdown_tx.send（SIGTERM）失败（receiver 可能已 drop）：{e:?}");
            }
        });
    }

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("正在优雅关闭 Agent...");
                agent.close().await.ok();
                info!("Agent 已关闭");
                break Ok(());
            }
            result = agent.run() => {
                if let Err(e) = result {
                    error!("Agent run() 错误: {}", e);
                }
                info!("Agent run() completed, restarting...");
            }
        }
    }
}

pub(crate) struct ServerSetup {
    pub(crate) server_msg_tx: tokio::sync::broadcast::Sender<DownstreamMessage>,
    pub(crate) shared_state: Arc<WsSharedState>,
    pub(crate) api_state: Arc<cyber_jianghu_agent::infra::api::HttpApiState>,
    pub(crate) actual_port: u16,
}

#[derive(Clone)]
struct LateClawSetup {
    shared_state: Arc<WsSharedState>,
    api_state: Arc<cyber_jianghu_agent::infra::api::HttpApiState>,
    server_msg_tx: tokio::sync::broadcast::Sender<DownstreamMessage>,
}

/// 统一的注册回调配置（Cognitive + Claw 模式共享）
/// Claw 模式独有字段为 Option，Cognitive 传入 None
struct CallbackSetup {
    /// WsSharedState — 仅 Claw 模式有值
    shared_state: Option<Arc<WsSharedState>>,
    api_state: Arc<cyber_jianghu_agent::infra::api::HttpApiState>,
    /// Downstream message tx — 仅 Claw 模式有值
    server_msg_tx: Option<tokio::sync::broadcast::Sender<DownstreamMessage>>,
    /// Runtime agent_id Arc — MUST be kept in sync with HttpApiState.agent_id.
    runtime_agent_id: Arc<RwLock<Uuid>>,
    persona_info: Option<cyber_jianghu_agent::soul::reflector::PersonaInfo>,
}
