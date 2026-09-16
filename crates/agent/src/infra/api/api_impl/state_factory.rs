//! HttpApiState 工厂与 DialogueEventHandler（自 mod.rs 外移）

use super::*;

struct NoopDialogueHandler;

impl DialogueEventHandler for NoopDialogueHandler {
    // 使用默认空实现即可
}

// ============================================================================
// 公共 API
// ============================================================================

/// 创建 HTTP 决策状态和 API 状态
///
/// # 参数
///
/// - `agent_id`: 当前 Agent ID (共享引用，注册后会被更新)
/// - `server_http_url`: Server HTTP URL（用于角色注册等 API 调用）
/// - `device_config`: 设备配置（device_id + auth_token）
/// - `server_dir`: 服务器配置目录路径
/// - `character_dir`: 角色配置目录路径
///
/// # 返回值
///
/// - `(Arc<HttpDecisionState>, HttpApiState)`: 决策状态和 API 状态
///
/// 初始化策略：
/// - 关系存储：使用默认数据库路径初始化，失败则为 None
/// - 记忆管理器：使用默认配置初始化（语义搜索已实现，见 SemanticMemoryBackend）
/// - 寿命计算器：使用默认配置强制初始化
/// - 对话客户端：使用空操作处理器强制初始化
/// - 意图验证器：使用默认规则引擎验证器强制初始化
///
/// # 注意
/// agent_id 是共享的，WebSocket 注册后会更新为服务器分配的真正 ID
///
/// # Arguments
/// * `config_path` - 配置文件完整路径（由调用者传入，确保与主程序一致）
#[allow(clippy::too_many_arguments)]
pub fn create_http_state(
    agent_id: Arc<RwLock<Uuid>>,
    server_http_url: String,
    server_ws_url: String,
    device_config: Option<crate::config::DeviceConfig>,
    server_dir: PathBuf,
    character_dir: PathBuf,
    reconnect_tx: Option<broadcast::Sender<ReconnectRequest>>,
    config_path: PathBuf,
    ws_shared_state: Option<Arc<crate::runtime::claw::WsSharedState>>,
    runtime_mode: crate::config::RuntimeMode,
    actual_port: u16,
) -> (Arc<HttpDecisionState>, HttpApiState) {
    // intent 通道：单 consumer (IntentWorker)，100 = 单 tick 内最大积压量
    let (intent_tx, intent_rx) = mpsc::channel(100);

    // 读取 agent_id（使用 block_in_place 在同步上下文中读取异步锁）
    let current_agent_id = {
        let guard = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(agent_id.read())
        });
        *guard
    }; // guard 在这里释放

    // 初始化数据目录（server-scoped）
    let data_dir = if !current_agent_id.is_nil() {
        character_dir
            .join(current_agent_id.to_string())
            .join("data")
    } else {
        server_dir.join("data")
    };

    // 预建目录：各 DB 模块的 open() 依赖此目录存在
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        tracing::error!("初始化失败: 无法创建数据目录 {:?} - {}", data_dir, e);
    }

    // 初始化关系存储（仅在有有效 agent_id 时创建）
    let relationship_store = if current_agent_id.is_nil() {
        None
    } else {
        RelationshipStore::open(
            current_agent_id,
            &data_dir.join(format!("relationships_{}.db", current_agent_id)),
        )
        .ok()
        .map(Arc::new)
    };
    let relationship_store = Arc::new(std::sync::RwLock::new(relationship_store));

    // 初始化记忆管理器（语义搜索已实现，见 SemanticMemoryBackend）
    let memory_config_template = MemoryManagerConfig {
        agent_id: current_agent_id,
        db_dir: data_dir.clone(),
        ..Default::default()
    };
    let memory_manager = MemoryManager::new(memory_config_template.clone())
        .ok()
        .map(|m| Arc::new(tokio::sync::RwLock::new(m)));
    let memory_manager = Arc::new(tokio::sync::RwLock::new(memory_manager));

    // 初始化对话客户端（使用空操作处理器）
    // 实际对话事件处理由外部系统通过 API 完成
    let dialogue_handler = Arc::new(NoopDialogueHandler);
    let dialogue_client = Some(Arc::new(DialogueClient::new(
        current_agent_id,
        dialogue_handler,
    )));

    // 初始化统一意图审查器（默认为空，待 Agent 启动后注入 ReflectorSoul）
    let intent_validator = None;

    let narrative_config = {
        let narrative_path = crate::config::config_dir().join("narrative_config.json");
        if narrative_path.exists() {
            std::fs::read_to_string(&narrative_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
        } else {
            None
        }
    };

    // death_event: 100 = 并发在线 agent 数量上限; tick_update: 64 = 每个 tick 周期的订阅者上限
    let (death_event_tx, _) = broadcast::channel(100);
    let (tick_update_tx, _) = broadcast::channel(64);

    // 预注册当前角色的记录器
    let soul_cycle_registrar = Arc::new(RwLock::new(HashMap::new()))
        as Arc<RwLock<HashMap<Uuid, Arc<soul_cycle_recorder::SoulCycleRecorder>>>>;
    if !current_agent_id.is_nil() {
        let db_path = data_dir.join(format!("soul_cycle_{}.db", current_agent_id));
        match soul_cycle_recorder::SoulCycleRecorder::open(current_agent_id, &db_path) {
            Ok(recorder) => {
                let recorder = Arc::new(recorder);
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        soul_cycle_registrar
                            .write()
                            .await
                            .insert(current_agent_id, recorder);
                    })
                });
            }
            Err(e) => {
                tracing::error!(
                    "[soul_cycle] 预注册失败: agent={}, path={:?}, error={}",
                    current_agent_id,
                    db_path,
                    e
                );
            }
        }
    }
    let data_dir_clone = data_dir.clone();

    let (auto_rebirth_init, llm_disabled_init, update_config) =
        crate::config::Config::from_file(&config_path)
            .map(|c| {
                (
                    c.runtime.auto_rebirth,
                    c.runtime.llm_disabled,
                    c.update.clone(),
                )
            })
            .unwrap_or((true, false, crate::config::UpdateConfig::default()));

    // 将持久化的 llm_disabled 同步到运行时全局标志，保持与 auto_rebirth 的读写对称
    crate::component::llm::direct_client::set_llm_disabled(llm_disabled_init);

    let api_state = HttpApiState {
        current_state: Arc::new(RwLock::new(None)),
        last_state_update: Arc::new(RwLock::new(None)),
        intent_tx: intent_tx.clone(),
        agent_id,
        tick_duration_secs: Arc::new(std::sync::atomic::AtomicU64::new(60)), // 默认 60 秒，注册后更新
        server_http_url: Arc::new(RwLock::new(server_http_url)),
        server_ws_url: Arc::new(RwLock::new(server_ws_url)),
        device_config: Arc::new(RwLock::new(device_config)),
        server_dir: Arc::new(RwLock::new(server_dir)),
        character_dir: Arc::new(RwLock::new(character_dir)),
        config_path,
        dialogue_client,
        relationship_store,
        memory_manager,
        memory_config_template: Some(memory_config_template),
        intent_validator,
        game_rules: Arc::new(RwLock::new(None)),
        narrative_generator: None,
        dynamic_persona: std::sync::Arc::new(std::sync::RwLock::new(None)),
        soul_cycle_registrar: soul_cycle_registrar.clone(),
        data_dir: data_dir_clone.clone(),
        dream_store: Some(Arc::new(RwLock::new(DreamState::default()))),
        reconnect_tx,
        death_event_tx,
        tick_update_tx,
        runtime_mode,
        narrative_config: Arc::new(RwLock::new(narrative_config)),
        is_dead: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        rebirth_delay_ticks: std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0)),
        rebirth_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
        pending_rebirth_agent_id: Arc::new(RwLock::new(None)),
        pending_rebirth_system_prompt: Arc::new(RwLock::new(None)),
        auto_rebirth: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(auto_rebirth_init)),
        actual_port,
        auto_register_deadline: Arc::new(RwLock::new(None)),
        llm_container: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        decision_context_snapshot: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        world_state_store: Arc::new(std::sync::RwLock::new(None)),
        updater: std::sync::Arc::new(crate::infra::updater::Updater::new(update_config)),
    };

    let decision_state = Arc::new(HttpDecisionState {
        api_state: api_state.clone(),
        intent_rx: Arc::new(Mutex::new(intent_rx)),
        ws_shared_state,
    });

    (decision_state, api_state)
}

/// HttpApiState 构建器（用于设置 AI 组件）
impl HttpApiState {
    /// 设置对话客户端
    pub fn with_dialogue_client(mut self, client: Arc<DialogueClient>) -> Self {
        self.dialogue_client = Some(client);
        self
    }

    /// 设置关系存储
    pub fn with_relationship_store(mut self, store: Arc<RelationshipStore>) -> Self {
        self.relationship_store = Arc::new(std::sync::RwLock::new(Some(store)));
        self
    }

    /// 设置记忆管理器
    pub fn with_memory_manager(mut self, manager: MemoryManager) -> Self {
        self.memory_manager = Arc::new(tokio::sync::RwLock::new(Some(Arc::new(
            tokio::sync::RwLock::new(manager),
        ))));
        self
    }

    /// 设置意图验证器
    pub fn with_intent_validator(mut self, validator: Arc<dyn Validator>) -> Self {
        self.intent_validator = Some(validator);
        self
    }

    /// 设置叙事生成器
    pub fn with_narrative_generator(mut self, generator: NarrativeGenerator) -> Self {
        self.narrative_generator = Some(Arc::new(generator));
        self
    }

    /// 设置动态人设
    /// 注入动态人设（run_agent 在 persona 创建后调用；切角色重建 persona 后同样适用）。
    /// std RwLock 仅短临界区写入，不跨 await 持有。
    pub fn set_dynamic_persona(&self, persona: ThreadSafePersona) {
        *self.dynamic_persona.write().expect("rwlock poisoned") = Some(persona);
    }

    /// 设置托梦存储
    pub fn with_dream_store(mut self, store: Arc<RwLock<DreamState>>) -> Self {
        self.dream_store = Some(store);
        self
    }

    /// 更新 Tick 持续时间（从 GameRules 获取后调用）
    pub fn set_tick_duration(&self, secs: u64) {
        use std::sync::atomic::Ordering;
        self.tick_duration_secs.store(secs, Ordering::Relaxed);
        tracing::info!("[http] Updated tick_duration to {}s", secs);
    }

    /// 读取当前托梦内容（不消费 — 不减少 remaining_ticks）
    ///
    /// 供 HTTP API handler 使用，lifecycle.rs 使用 consume_dream() 进行实际消费。
    /// 前提：consume_dream() 已在当前 tick 调用过（由 lifecycle run_cycle 保证），
    /// 因此 dream 数据已从磁盘加载到内存。使用 read lock 不阻塞消费端。
    pub async fn peek_dream(&self) -> Option<String> {
        let dream_store = self.dream_store.as_ref()?;
        let dream = dream_store.read().await;
        if dream.remaining_ticks > 0 {
            dream.thought.clone()
        } else {
            None
        }
    }

    /// 获取当前托梦内容（如果有）
    /// 每次调用会减少剩余回合数
    pub async fn consume_dream(&self) -> Option<String> {
        let dream_store = self.dream_store.as_ref()?;
        let mut dream = dream_store.write().await;

        let agent_id = *self.agent_id.read().await;
        let dream_dir = self
            .character_dir
            .read()
            .await
            .join(agent_id.to_string())
            .join("data");
        dream.ensure_loaded(&dream_dir, &agent_id);

        let mut changed = false;

        let result = if dream.remaining_ticks > 0 {
            let thought = dream.thought.clone();
            dream.remaining_ticks = dream.remaining_ticks.saturating_sub(1);
            changed = true;

            if dream.remaining_ticks == 0 {
                info!("[dream] 托梦效果已结束");
                dream.thought = None;
            }

            thought
        } else {
            if dream.thought.is_some() {
                dream.thought = None;
                changed = true;
            }
            None
        };

        if changed {
            dream.save_to_file(&dream_dir, &agent_id);
        }

        result
    }

    /// 将 tick 到达广播给 SSE 订阅者（/api/v1/state/stream）
    ///
    /// 唯一调用方：lifecycle `update_tick_state`（每个到达的 WorldState 必经之路）。
    /// 修复点：此前 tick_update_tx 的唯一 send 点位于无调用方的 http_decision 死代码中，
    /// SSE 流在订阅首发一帧后永久沉默，下游世界视图冻结。
    /// 无订阅者时不发送也不告警——面板未连接是常态而非错误（R7：无异常可吞）。
    pub fn notify_tick_observers(&self, world_state: &WorldState) {
        if self.tick_update_tx.receiver_count() > 0
            && let Err(e) = self.tick_update_tx.send(world_state.tick_id)
        {
            tracing::warn!("tick_update_tx.send 失败（receiver 可能已 drop）：{e:?}");
        }
    }

    /// 在 Tick 处理后异步更新关系描述
    pub async fn maybe_update_narratives(&self, world_state: &WorldState) {
        let Some(generator) = &self.narrative_generator else {
            return; // 没有 LlmClient，跳过
        };

        let store_guard = self.relationship_store.read().expect("rwlock poisoned");
        let Some(store) = store_guard.as_ref() else {
            return;
        };

        let persona = self
            .dynamic_persona
            .read()
            .expect("rwlock poisoned")
            .clone();
        let Some(persona) = persona else {
            return;
        };

        let current_tick = world_state.tick_id;

        // 异步更新所有附近实体的关系描述
        for entity in &world_state.entities {
            let target_id = entity.id;

            // 获取关系记忆
            let memory = match store.get_relationship(target_id) {
                Ok(Some(m)) => m,
                _ => continue,
            };

            // 克隆需要的数据
            let generator = generator.clone();
            let store_clone = store.clone();
            let persona_clone = persona.read(|p| p.clone());

            // 异步更新（不阻塞主流程）
            tokio::spawn(async move {
                let _ = generator
                    .update_with_debounce(
                        target_id,
                        current_tick,
                        &memory,
                        &persona_clone,
                        &store_clone,
                    )
                    .await;
            });
        }
    }

    /// 刷新设备认证令牌（HTTP 401 时调用）
    ///
    /// 调用 `POST {server_http_url}/api/v1/device/verify` 严格校验设备是否仍被持有。
    /// - 200 → 用 server 返回的 token 替换本地 token
    /// - 404 → 设备不存在（DB 被清空等场景），返回错误让上层走 ensure_device 重新注册
    /// - 其他 → 错误传播
    ///
    /// **narrative_config 不在此处同步**：它是"游戏规则"数据，归属 character_register
    /// 路径。设备身份端点不负责游戏规则下发。
    pub async fn refresh_auth_token(&self) -> anyhow::Result<()> {
        // 1. 获取当前设备配置
        //    guard 必须限定在本作用域内释放：函数尾部要取写锁保存新 token，
        //    若读锁存活到写锁请求处，同一任务自等自释放 → 死锁
        let device_id = {
            let device = self.device_config.read().await;
            device.as_ref().context("设备身份未初始化")?.device_id
        };

        // 2. 获取 HTTP URL
        let http_url = self.server_http_url.read().await.clone();
        let url = format!("{}/api/v1/device/verify", http_url);

        // 3. 调 /device/verify 严格校验
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .json(&serde_json::json!({ "device_id": device_id }))
            .send()
            .await
            .context("刷新令牌请求失败")?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            // server 不认这个 device → 触发上层重启以走 ensure_device 重新注册
            anyhow::bail!(
                "device {} 已不被 server 认可，需重启走 ensure_device",
                device_id
            );
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("刷新令牌失败 {}: {}", status, body);
        }

        #[derive(Deserialize)]
        struct VerifyResponse {
            device_id: Uuid,
            auth_token: String,
        }

        let result: VerifyResponse = response.json().await.context("解析刷新令牌响应失败")?;

        // 防御：server 绝不应回不同 device_id，若发生则 fail-fast
        if result.device_id != device_id {
            anyhow::bail!(
                "server 返回 device_id {} 与请求 {} 不一致",
                result.device_id,
                device_id
            );
        }

        info!("设备 {} 的令牌刷新成功", device_id);

        // 4. 更新 device_config 并持久化
        let mut device_guard = self.device_config.write().await;
        if let Some(ref mut device) = *device_guard {
            device.auth_token = result.auth_token.clone();
            let server_dir = self.server_dir.read().await;
            let device_path = server_dir.join("device.yaml");
            if let Some(parent) = device_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if let Err(e) = device.save_to_file(&device_path) {
                error!("持久化刷新后的令牌失败: {}", e);
            }
        }

        Ok(())
    }

    /// 获取指定角色的三魂记录器（按需加载）
    ///
    /// 如果记录器已缓存则直接返回，否则从磁盘加载对应角色的 SQLite 文件。
    pub async fn soul_recorder_for(
        &self,
        agent_id: Uuid,
    ) -> Option<Arc<soul_cycle_recorder::SoulCycleRecorder>> {
        // 1. 检查缓存
        {
            let registrar = self.soul_cycle_registrar.read().await;
            if let Some(recorder) = registrar.get(&agent_id) {
                return Some(recorder.clone());
            }
        }
        // 2. 按需加载/创建
        // 其他角色的数据在 character_dir/{agent_id}/data/soul_cycle_{agent_id}.db
        let character_dir = self.character_dir.read().await;
        let data_dir = character_dir.join(agent_id.to_string()).join("data");
        let db_path = data_dir.join(format!("soul_cycle_{}.db", agent_id));
        // 预建目录：SoulCycleRecorder::open 内部仅 create_dir_all(parent)，
        // 若中间目录链不完整仍会失败，此处确保完整路径存在
        if let Err(e) = std::fs::create_dir_all(&data_dir) {
            tracing::error!("[soul_cycle] 无法创建数据目录 {:?}: {}", data_dir, e);
            return None;
        }
        match soul_cycle_recorder::SoulCycleRecorder::open(agent_id, &db_path) {
            Ok(recorder) => {
                let recorder = Arc::new(recorder);
                let mut registrar = self.soul_cycle_registrar.write().await;
                registrar.insert(agent_id, recorder.clone());
                Some(recorder)
            }
            Err(e) => {
                tracing::error!("[soul_cycle] 懒加载失败: agent={}, error={}", agent_id, e);
                None
            }
        }
    }
}
