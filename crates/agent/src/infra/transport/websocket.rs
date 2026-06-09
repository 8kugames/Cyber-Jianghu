// ============================================================================
// WebSocket 客户端 - 纯 I/O 层
// ============================================================================
//
// 职责：
// - 连接管理（WebSocket）
// - 消息序列化/反序列化
// - 自动重连
//
// 不负责：
// - 业务逻辑
// - 决策
// - 验证

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use tracing::{debug, error, info, warn};
use url::Url;
use uuid::Uuid;

use cyber_jianghu_protocol::{
    ClientMessage, DialogueMessage, GameRules, Intent, ServerMessage, SkillContent,
    WorldBuildingRules, WorldState,
};

// 重导出 config 中的 ServerConfig
pub use crate::config::ServerConfig;

// ============================================================================
// ConnectError - 区分认证失败和其他连接错误
// ============================================================================

/// WebSocket connection error with auth failure distinction
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("Authentication failed (HTTP 400)")]
    AuthFailed,
    #[error("Connection failed: {0}")]
    ConnectionFailed(#[from] anyhow::Error),
}

// ============================================================================
// WebSocket 客户端
// ============================================================================

/// WebSocket 客户端（纯 I/O）
pub struct WebSocketClient {
    config: ServerConfig,
    /// 设备身份（device_id + auth_token）
    identity: Option<(Uuid, String)>,
    state: Arc<RwLock<ConnectionState>>,
}

/// 注册数据（后台任务收到 Registered 消息后存储）
struct RegistrationData {
    agent_id: Uuid,
    game_rules: GameRules,
    world_building_rules: Option<WorldBuildingRules>,
    agent_name: Option<String>,
    is_alive: bool,
    narrative_config: Option<cyber_jianghu_protocol::NarrativeConfig>,
    narrative_config_hash: Option<String>,
}

/// 实时意图执行结果
#[derive(Clone)]
pub struct ExecutionResultData {
    pub tick_id: i64,
    pub intent_id: Uuid,
    pub success: bool,
    pub error: Option<String>,
    pub state_change_summary: Option<String>,
}

/// 技能配置更新回调类型
pub type SkillUpdateCallback = Arc<dyn Fn(Vec<SkillContent>, Vec<String>) + Send + Sync>;

/// 叙事化配置更新回调类型
type NarrativeConfigCallback =
    Arc<dyn Fn(cyber_jianghu_protocol::NarrativeConfig, Option<String>) + Send + Sync>;

/// 连接状态
struct ConnectionState {
    connected: bool,
    /// Agent ID（注册后设置，即角色ID）
    agent_id: Option<Uuid>,
    /// 游戏规则
    game_rules: Option<GameRules>,
    /// 世界观规则
    world_building_rules: Option<WorldBuildingRules>,
    /// 游戏规则回调
    game_rules_callback: Option<Arc<dyn Fn(GameRules) + Send + Sync>>,
    /// 对话消息回调
    dialogue_callback: Option<Arc<dyn Fn(DialogueMessage) + Send + Sync>>,
    /// 世界观规则回调
    world_building_rules_callback: Option<Arc<dyn Fn(WorldBuildingRules) + Send + Sync>>,
    /// Server 消息透传回调（用于 OpenClaw 集成）
    server_msg_callback: Option<Arc<dyn Fn(ServerMessage) + Send + Sync>>,
    /// 动作配置更新回调
    action_update_callback: Option<Arc<dyn Fn(ServerMessage) + Send + Sync>>,
    /// 技能配置更新回调（ConfigUpdate with config_type="skills"）
    /// 参数: (skills, removed_items)
    skill_update_callback: Option<SkillUpdateCallback>,
    /// Prompt 模板配置更新回调（ConfigUpdate with config_type="prompt_templates"）
    /// 参数: (PromptTemplateConfig)
    prompt_template_callback:
        Option<Arc<dyn Fn(cyber_jianghu_protocol::PromptTemplateConfig) + Send + Sync>>,
    /// 上次收到的 prompt_templates content_hash（用于 skip-optimization）
    prompt_template_hash: Option<String>,
    /// WS 后台线程是否已成功投递 prompt_templates（用于 HTTP 拉取条件跳过）
    prompt_template_received: bool,
    /// 事件特质规则更新回调（ConfigUpdate with config_type="persona_event_rules"）
    /// 参数: Vec<TraitMappingRule>
    persona_event_rules_callback:
        Option<Arc<dyn Fn(Vec<crate::component::persona::TraitMappingRule>) + Send + Sync>>,
    /// 叙事化配置更新回调（ConfigUpdate with config_type="narrative_config"）
    /// 参数: (NarrativeConfig, Option<content_hash>)
    narrative_config_callback: Option<NarrativeConfigCallback>,

    // ---- 后台任务架构 ----
    /// 后台 WebSocket 任务句柄
    reader_task: Option<tokio::task::JoinHandle<()>>,
    /// 关闭信号（broadcast，支持一次性触发）
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
    /// Intent 发送通道（主循环 → 后台任务）
    intent_tx: Option<tokio::sync::mpsc::Sender<ClientMessage>>,
    /// WorldState 通道（后台任务 → 主循环，watch 保留最新值）
    worldstate_tx: Option<tokio::sync::watch::Sender<Option<WorldState>>>,
    /// 注册通知通道（后台任务 → 主循环）
    registered_tx: Option<tokio::sync::watch::Sender<Option<RegistrationData>>>,
    /// ExecutionResult 通道（后台任务 → 主循环，mpsc 保留全部结果）
    execution_result_tx: Option<tokio::sync::mpsc::Sender<ExecutionResultData>>,
    /// ExecutionResult 接收端（Arc<Mutex> 允许 &self 下异步访问）
    execution_result_rx: Option<
        std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<ExecutionResultData>>>,
    >,
}

impl WebSocketClient {
    /// 创建新的客户端
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            identity: None,
            state: Arc::new(RwLock::new(ConnectionState {
                connected: false,
                agent_id: None,
                game_rules: None,
                world_building_rules: None,
                game_rules_callback: None,
                dialogue_callback: None,
                world_building_rules_callback: None,
                server_msg_callback: None,
                action_update_callback: None,
                skill_update_callback: None,
                prompt_template_callback: None,
                prompt_template_hash: None,
                prompt_template_received: false,
                persona_event_rules_callback: None,
                narrative_config_callback: None,
                reader_task: None,
                shutdown_tx: None,
                intent_tx: None,
                worldstate_tx: None,
                registered_tx: None,
                execution_result_tx: None,
                execution_result_rx: None,
            })),
        }
    }

    /// 设置设备身份
    pub fn set_identity(&mut self, device_id: Uuid, auth_token: String) {
        self.identity = Some((device_id, auth_token));
    }

    /// 更新服务器 URL（用于热切换）
    pub fn update_server_url(&mut self, ws_url: String, http_url: String) {
        self.config.ws_url = ws_url;
        self.config.http_url = http_url;
    }

    /// 连接到服务器
    pub async fn connect(&self) -> Result<(), ConnectError> {
        let (device_id, auth_token) = self.identity.as_ref().ok_or_else(|| {
            ConnectError::ConnectionFailed(anyhow::anyhow!(
                "Identity not set. Call set_identity() first."
            ))
        })?;

        // 读取 agent_id（如果有）
        let agent_id_opt = self.agent_id();

        let url_with_token = self
            .config
            .ws_url_with_token(*device_id, auth_token, agent_id_opt);
        let url =
            Url::parse(&url_with_token).map_err(|e| ConnectError::ConnectionFailed(e.into()))?;

        info!("Connecting to {}", self.config.ws_url);

        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((ws, _)) => {
                let mut state = self.state.write().await;

                // 创建通道
                let (intent_tx, intent_rx) = tokio::sync::mpsc::channel(32);
                let (worldstate_tx, _) = tokio::sync::watch::channel(None);
                let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);
                let (registered_tx, _) = tokio::sync::watch::channel(None);
                let (execution_result_tx, execution_result_rx) = tokio::sync::mpsc::channel(16);

                // 启动后台 WebSocket 任务（独占 ws）
                let state_arc = self.state.clone();
                let handle = tokio::spawn(async move {
                    websocket_background_task(ws, state_arc, intent_rx, shutdown_rx).await;
                });

                // 更新状态
                state.connected = true;
                state.agent_id = None;
                state.game_rules = None;
                state.world_building_rules = None;
                state.reader_task = Some(handle);
                state.shutdown_tx = Some(shutdown_tx);
                state.intent_tx = Some(intent_tx);
                state.worldstate_tx = Some(worldstate_tx);
                state.registered_tx = Some(registered_tx);
                state.execution_result_tx = Some(execution_result_tx);
                state.execution_result_rx = Some(std::sync::Arc::new(tokio::sync::Mutex::new(
                    execution_result_rx,
                )));

                info!("Connected to server (background task started)");
                Ok(())
            }
            Err(tokio_tungstenite::tungstenite::Error::Http(resp))
                if matches!(resp.status().as_u16(), 400 | 401) =>
            {
                warn!("WebSocket auth failed (HTTP {})", resp.status().as_u16());
                Err(ConnectError::AuthFailed)
            }
            Err(e) => Err(ConnectError::ConnectionFailed(anyhow::anyhow!(
                "Failed to connect to WebSocket server: {}",
                e
            ))),
        }
    }

    /// 设置指定的 Agent ID（用于热切换）
    pub fn set_agent_id(&self, agent_id: Option<Uuid>) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.agent_id = agent_id;
            });
        });
    }

    /// 获取 Agent ID
    pub fn agent_id(&self) -> Option<Uuid> {
        // 使用 try_read 避免异步
        self.state.try_read().ok()?.agent_id
    }

    /// 等待 Agent ID 可用（注册后）
    pub async fn wait_for_agent_id(&self) -> Result<Uuid> {
        // 尝试读取 agent_id，如果还没有就等待一小段时间后重试
        for _ in 0..10 {
            if let Some(id) = self.agent_id() {
                return Ok(id);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        Err(anyhow::anyhow!("Agent ID not available after registration"))
    }

    /// 设置游戏规则回调
    pub fn set_game_rules_callback(&self, callback: Arc<dyn Fn(GameRules) + Send + Sync>) {
        // 使用 block_in_place 在同步上下文中修改状态
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.game_rules_callback = Some(callback);
            });
        });
    }

    /// 设置对话消息回调
    pub fn set_dialogue_callback(&self, callback: Arc<dyn Fn(DialogueMessage) + Send + Sync>) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.dialogue_callback = Some(callback);
            });
        });
    }

    /// 设置世界观规则回调
    pub fn set_world_building_rules_callback(
        &self,
        callback: Arc<dyn Fn(WorldBuildingRules) + Send + Sync>,
    ) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.world_building_rules_callback = Some(callback);
            });
        });
    }

    /// 设置 Server 消息透传回调（用于 OpenClaw 集成）
    ///
    /// 当收到 Server 下行消息时，此回调会被调用，允许 OpenClaw
    /// 实时接收 Server 的所有消息（错误、对话、规则更新等）
    pub fn set_server_msg_callback(&self, callback: Arc<dyn Fn(ServerMessage) + Send + Sync>) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.server_msg_callback = Some(callback);
            });
        });
    }

    /// 获取当前 server_msg_callback（用于 callback chaining）
    pub fn get_server_msg_callback(&self) -> Option<Arc<dyn Fn(ServerMessage) + Send + Sync>> {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let state = self.state.read().await;
                state.server_msg_callback.clone()
            })
        })
    }

    /// 设置动作配置更新回调
    pub fn set_action_update_callback(&self, callback: Arc<dyn Fn(ServerMessage) + Send + Sync>) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.action_update_callback = Some(callback);
            });
        });
    }

    /// 设置技能配置更新回调（ConfigUpdate with config_type="skills"）
    /// 参数: (skills, removed_items)
    pub fn set_skill_update_callback(&self, callback: SkillUpdateCallback) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.skill_update_callback = Some(callback);
            });
        });
    }

    /// 设置 Prompt 模板配置更新回调（ConfigUpdate with config_type="prompt_templates"）
    /// 参数: (PromptTemplateConfig)
    pub fn set_prompt_template_callback(
        &self,
        callback: Arc<dyn Fn(cyber_jianghu_protocol::PromptTemplateConfig) + Send + Sync>,
    ) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.prompt_template_callback = Some(callback);
            });
        });
    }

    /// 检查 WS 后台线程是否已成功投递 prompt_templates
    pub fn is_prompt_template_received(&self) -> bool {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async { self.state.read().await.prompt_template_received })
        })
    }

    /// 设置事件特质规则更新回调（ConfigUpdate with config_type="persona_event_rules"）
    pub fn set_persona_event_rules_callback(
        &self,
        callback: Arc<dyn Fn(Vec<crate::component::persona::TraitMappingRule>) + Send + Sync>,
    ) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.persona_event_rules_callback = Some(callback);
            });
        });
    }

    /// 设置叙事化配置更新回调（ConfigUpdate with config_type="narrative_config"）
    pub fn set_narrative_config_callback(&self, callback: NarrativeConfigCallback) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.narrative_config_callback = Some(callback);
            });
        });
    }

    /// 等待注册响应
    ///
    /// 返回值：
    /// - `Ok(Some((agent_id, game_rules, world_building_rules, agent_name, is_alive, narrative_config, narrative_config_hash)))` - 有角色
    /// - `Ok(None)` - 无角色，等待注册（agent_id 为 nil）
    /// - `Err(e)` - 连接错误
    #[allow(clippy::type_complexity)]
    pub async fn wait_for_registration(
        &self,
    ) -> Result<
        Option<(
            Uuid,
            GameRules,
            Option<WorldBuildingRules>,
            Option<String>,
            bool,
            Option<cyber_jianghu_protocol::NarrativeConfig>,
            Option<String>,
        )>,
    > {
        let mut rx = {
            let state = self.state.read().await;
            state
                .registered_tx
                .as_ref()
                .context("Not connected to server")?
                .subscribe()
        };

        loop {
            // 检查当前值（避免竞态）
            match rx.borrow().as_ref() {
                // agent_id 为 nil → 需要注册新角色
                Some(data) if data.agent_id == Uuid::nil() => {
                    info!("已连接服务器，等待角色注册...");
                    return Ok(None);
                }
                Some(data) => {
                    info!(
                        "Agent registered with ID: {}, alive={}",
                        data.agent_id, data.is_alive
                    );
                    // 更新 state 中的字段
                    let mut state = self.state.write().await;
                    state.agent_id = Some(data.agent_id);
                    state.game_rules = Some(data.game_rules.clone());
                    if let Some(ref rules) = data.world_building_rules {
                        state.world_building_rules = Some(rules.clone());
                    }
                    return Ok(Some((
                        data.agent_id,
                        data.game_rules.clone(),
                        data.world_building_rules.clone(),
                        data.agent_name.clone(),
                        data.is_alive,
                        data.narrative_config.clone(),
                        data.narrative_config_hash.clone(),
                    )));
                }
                None => {} // 尚未收到注册消息
            }

            // 等待值变化
            if rx.changed().await.is_err() {
                let state = self.state.read().await;
                if !state.connected {
                    bail!("Connection closed during registration");
                }
            }
        }
    }

    /// 接收 WorldState（阻塞直到收到新值）
    pub async fn receive_world_state(&self) -> Result<WorldState> {
        let mut rx = {
            let state = self.state.read().await;
            state
                .worldstate_tx
                .as_ref()
                .context("Not connected to server")?
                .subscribe()
        };

        // 阻塞等待 sender 发送新值（每个 tick 广播一次）
        rx.changed().await.context("WorldState channel closed")?;

        rx.borrow()
            .as_ref()
            .cloned()
            .context("WorldState channel produced None")
    }

    /// 接收 ExecutionResult（非阻塞，返回所有已缓存结果）
    ///
    /// mpsc channel 保留全部结果。非阻塞 drain，返回空 Vec 表示无结果。
    pub async fn try_receive_execution_result(&self) -> Result<Vec<ExecutionResultData>> {
        let rx_arc = {
            let state = self.state.read().await;
            state
                .execution_result_rx
                .as_ref()
                .context("Not connected to server")?
                .clone()
        };

        let mut rx = rx_arc.lock().await;
        let mut results = Vec::new();
        while let Ok(data) = rx.try_recv() {
            results.push(data);
        }
        Ok(results)
    }

    /// 等待 ExecutionResult（阻塞等待，带超时，收集全部多 intent 结果）
    ///
    /// 等待首个结果（带超时），然后 drain 通道中剩余结果。
    /// 返回空 Vec 表示超时无结果。
    pub async fn wait_for_execution_result(
        &self,
        timeout_ms: u64,
    ) -> Result<Vec<ExecutionResultData>> {
        let rx_arc = {
            let state = self.state.read().await;
            state
                .execution_result_rx
                .as_ref()
                .context("Not connected to server")?
                .clone()
        };

        let mut rx = rx_arc.lock().await;

        // 先 drain 已缓存的结果（可能 server 已发送完毕）
        let mut results = Vec::new();
        while let Ok(data) = rx.try_recv() {
            results.push(data);
        }
        if !results.is_empty() {
            return Ok(results);
        }

        // 无缓存结果，等待首个结果（带超时）
        match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), rx.recv()).await {
            Ok(Some(first)) => {
                results.push(first);
                // drain 后续结果（server 连续发送，应该已在缓冲区）
                while let Ok(data) = rx.try_recv() {
                    results.push(data);
                }
                Ok(results)
            }
            Ok(None) => anyhow::bail!("ExecutionResult channel closed"),
            Err(_) => Ok(Vec::new()), // timeout
        }
    }

    /// 发送 Intent（通过 mpsc channel → 后台任务）
    pub async fn send_intent(&self, intent: &Intent) -> Result<()> {
        let tx = {
            let state = self.state.read().await;
            state
                .intent_tx
                .as_ref()
                .context("Not connected to server")?
                .clone()
        };

        tx.send(ClientMessage::from_intent(intent.clone()))
            .await
            .context("Failed to send intent to background task")?;

        debug!("Sent Intent to background: {:?}", intent.action_type);
        Ok(())
    }

    /// 获取 Intent 发送端的 clone（用于绑定到 ImmediateEventHandler）
    pub async fn intent_sender(&self) -> Option<tokio::sync::mpsc::Sender<ClientMessage>> {
        let state = self.state.read().await;
        state.intent_tx.clone()
    }

    /// 发送三魂循环元数据到服务器（fire-and-forget，通过统一通道）
    pub async fn send_soul_cycle_report(
        &self,
        tick_id: i64,
        pipe_seq: i32,
        metadata: cyber_jianghu_protocol::SoulCycleMetadata,
    ) -> Result<()> {
        let agent_id = self.agent_id();
        let msg = ClientMessage::SoulCycleReport {
            tick_id,
            agent_id,
            pipe_seq,
            metadata,
        };
        let tx = {
            let state = self.state.read().await;
            state
                .intent_tx
                .as_ref()
                .context("Not connected to server")?
                .clone()
        };
        tx.send(msg)
            .await
            .context("Failed to send soul cycle report")?;
        Ok(())
    }

    /// 发送每日 LLM 日志摘要到服务器
    pub async fn send_daily_summary(&self, game_day: i64, summary: &str) -> Result<()> {
        let msg = ClientMessage::DailySummary {
            game_day,
            summary: summary.to_string(),
        };
        let tx = {
            let state = self.state.read().await;
            state
                .intent_tx
                .as_ref()
                .context("Not connected to server")?
                .clone()
        };
        tx.send(msg).await.context("Failed to send daily summary")?;
        Ok(())
    }

    pub fn game_rules(&self) -> Option<GameRules> {
        self.state.try_read().ok()?.game_rules.clone()
    }

    /// 获取世界观规则
    pub fn world_building_rules(&self) -> Option<WorldBuildingRules> {
        self.state.try_read().ok()?.world_building_rules.clone()
    }

    /// 检查是否已连接
    pub async fn is_connected(&self) -> bool {
        self.state.read().await.connected
    }

    /// 断开连接
    pub async fn disconnect(&self) {
        let handle = {
            let mut state = self.state.write().await;

            // 发送关闭信号
            if let Some(tx) = state.shutdown_tx.take() {
                let _ = tx.send(());
            }

            let handle = state.reader_task.take();
            state.connected = false;
            state.intent_tx = None;
            state.worldstate_tx = None;
            state.registered_tx = None;
            state.execution_result_tx = None;
            state.execution_result_rx = None;

            handle
        };

        // 等待后台任务结束（带超时）
        if let Some(handle) = handle {
            match tokio::time::timeout(tokio::time::Duration::from_secs(5), handle).await {
                Ok(Ok(())) => debug!("Background task shutdown cleanly"),
                Ok(Err(e)) => warn!("Background task error on shutdown: {}", e),
                Err(_) => {
                    warn!("Background task shutdown timeout, aborting");
                }
            }
        }

        info!("Disconnected from server");
    }
}

// ============================================================================
// 后台 WebSocket 任务
// ============================================================================

/// 后台 WebSocket 任务
///
/// 独占 WebSocket，使用 tokio::select! 同时处理：
/// - 接收消息（持续轮询 ws.next()，自动响应 Ping/Pong）
/// - 发送 intent（通过 mpsc channel）
///
/// WorldState 通过 watch channel 传递给主循环，
/// 其他消息通过回调处理（与原 receive_and_handle_message 逻辑一致）。
async fn websocket_background_task(
    mut ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    state: Arc<RwLock<ConnectionState>>,
    mut intent_rx: tokio::sync::mpsc::Receiver<ClientMessage>,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    /// 读超时：server 每 30s 发 Ping，120s 无任何消息 = 连接已死
    const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

    info!("WebSocket background task started");
    let mut last_message_time = std::time::Instant::now();

    loop {
        let remaining = READ_TIMEOUT.saturating_sub(last_message_time.elapsed());

        tokio::select! {
            // 读超时：连接静默死亡（server 重启、网络断开、TCP 半开）
            _ = tokio::time::sleep(remaining) => {
                warn!(
                    "Background: 读超时 ({:?} 无消息)，连接已死",
                    last_message_time.elapsed()
                );
                if let Some(ref tx) = {
                    let guard = state.read().await;
                    guard.worldstate_tx.clone()
                } {
                    let _ = tx.send(None);
                }
                break;
            }

            // 检查关闭信号
            res = shutdown_rx.recv() => {
                match res {
                    Ok(_) => {
                        info!("WebSocket background: shutdown signal received");
                        // 优雅关闭 WebSocket
                        let _ = ws.close(None).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Lagged shutdown signal, ignore
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }

            // 接收消息（持续轮询，自动响应 Ping）
            msg_result = ws.next() => {
                last_message_time = std::time::Instant::now();
                match msg_result {
                    Some(Ok(Message::Text(text))) => {
                        // 克隆回调（避免在处理中持有锁）
                        let (game_rules_cb, dialogue_cb, wb_rules_cb, action_update_cb, skill_update_cb, prompt_template_cb, persona_event_rules_cb, narrative_config_cb, server_msg_cb, ws_tx, reg_tx, exec_result_tx) = {
                            let state_guard = state.read().await;
                            (
                                state_guard.game_rules_callback.clone(),
                                state_guard.dialogue_callback.clone(),
                                state_guard.world_building_rules_callback.clone(),
                                state_guard.action_update_callback.clone(),
                                state_guard.skill_update_callback.clone(),
                                state_guard.prompt_template_callback.clone(),
                                state_guard.persona_event_rules_callback.clone(),
                                state_guard.narrative_config_callback.clone(),
                                state_guard.server_msg_callback.clone(),
                                state_guard.worldstate_tx.clone(),
                                state_guard.registered_tx.clone(),
                                state_guard.execution_result_tx.clone(),
                            )
                        };

                        match serde_json::from_str::<ServerMessage>(&text) {
                            Ok(ServerMessage::WorldState { data }) => {
                                debug!("Background: WorldState tick={}", data.tick_id);
                                if let Some(ref tx) = ws_tx {
                                    let _ = tx.send(Some(data));
                                }
                            }
                            Ok(msg @ ServerMessage::ConfigUpdate { .. }) => {
                                if let ServerMessage::ConfigUpdate {
                                    ref config_type,
                                    ref update_type,
                                    ref version,
                                    ref content,
                                    ref content_hash,
                                    ref updated_items,
                                    ref removed_items,
                                } = msg
                                {
                                    info!(
                                        "Background: ConfigUpdate type={}, config_type={}, v={}, +{}, -{}",
                                        update_type, config_type, version,
                                        updated_items.len(), removed_items.len()
                                    );

                                    // 处理 skills 配置更新
                                    if config_type == "skills" {
                                        // 目前仅支持 full update_type，增量更新暂未实现
                                        if update_type != "full" {
                                            warn!(
                                                "ConfigUpdate: skills update_type={} not fully supported, treating as full",
                                                update_type
                                            );
                                        }

                                        if let Ok(skills) = serde_json::from_value::<Vec<SkillContent>>(content.clone()) {
                                            if let Some(ref cb) = skill_update_cb {
                                                cb(skills, removed_items.clone());
                                            }
                                        } else {
                                            warn!("Failed to parse skills content from ConfigUpdate");
                                        }
                                    // 处理 actions 配置更新
                                    // 当前仅支持 full update，增量更新暂未实现
                                    // actions 通过 action_update_callback 透传整个 ServerMessage
                                    } else if config_type == "actions" {
                                        if let Some(ref cb) = action_update_cb {
                                            cb(msg.clone());
                                        }
                                    // 处理 game_rules 配置更新
                                    } else if config_type == "game_rules" {
                                        if let Ok(game_rules) = serde_json::from_value::<GameRules>(content.clone()) {
                                            // 更新本地缓存
                                            {
                                                let mut guard = state.write().await;
                                                guard.game_rules = Some(game_rules.clone());
                                            }
                                            // 调用回调
                                            if let Some(ref cb) = game_rules_cb {
                                                cb(game_rules);
                                            }
                                        } else {
                                            warn!("Failed to parse game_rules content from ConfigUpdate");
                                        }
                                    // 处理 world_building_rules 配置更新
                                    } else if config_type == "world_building_rules" {
                                        if let Ok(wb_rules) = serde_json::from_value::<WorldBuildingRules>(content.clone()) {
                                            // 更新本地缓存
                                            {
                                                let mut guard = state.write().await;
                                                guard.world_building_rules = Some(wb_rules.clone());
                                            }
                                            // 调用回调
                                            if let Some(ref cb) = wb_rules_cb {
                                                cb(wb_rules);
                                            }
                                        } else {
                                            warn!("Failed to parse world_building_rules content from ConfigUpdate");
                                        }
                                    // 处理 prompt_templates 配置更新（JSON 格式 + hash skip）
                                    } else if config_type == "prompt_templates" {
                                        // hash skip: 内容未变则跳过更新
                                        let should_update = {
                                            let state_guard = state.read().await;
                                            match (content_hash.as_ref(), state_guard.prompt_template_hash.as_ref()) {
                                                (Some(new_hash), Some(old_hash)) => new_hash != old_hash,
                                                _ => true,
                                            }
                                        };
                                        if should_update {
                                            // 无论解析成功与否，先记录 hash，防止相同坏数据反复重试
                                            if let Some(hash) = content_hash.as_ref() {
                                                let mut state_guard = state.write().await;
                                                state_guard.prompt_template_hash = Some(hash.clone());
                                            }
                                            if let Ok(config) = cyber_jianghu_protocol::PromptTemplateConfig::from_json_value(content.clone()) {
                                                // 标记 WS 已成功投递，HTTP 拉取可跳过
                                                {
                                                    let mut state_guard = state.write().await;
                                                    state_guard.prompt_template_received = true;
                                                }
                                                if let Some(ref cb) = prompt_template_cb {
                                                    cb(config);
                                                }
                                            } else {
                                                warn!("Failed to parse prompt_templates JSON from ConfigUpdate");
                                            }
                                        } else {
                                            debug!("prompt_templates skip: hash unchanged");
                                        }
                                    // 处理 persona_event_rules 配置更新
                                    } else if config_type == "persona_event_rules" {
                                        #[derive(serde::Deserialize)]
                                        struct RulesJson {
                                            rules: Vec<crate::component::persona::TraitMappingRule>,
                                        }
                                        match serde_json::from_value::<RulesJson>(content.clone()) {
                                            Ok(parsed) => {
                                                if let Some(ref cb) = persona_event_rules_cb {
                                                    cb(parsed.rules);
                                                }
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "Failed to parse persona_event_rules content from ConfigUpdate: {}",
                                                    e
                                                );
                                            }
                                        }
                                    // 处理 narrative_config 配置更新
                                    } else if config_type == "narrative_config" {
                                        if let Ok(nc) = serde_json::from_value::<cyber_jianghu_protocol::NarrativeConfig>(content.clone()) {
                                            if let Some(ref cb) = narrative_config_cb {
                                                cb(nc, content_hash.clone());
                                            }
                                        } else {
                                            warn!("Failed to parse narrative_config content from ConfigUpdate");
                                        }
                                    }
                                }
                                if let Some(ref cb) = server_msg_cb {
                                    cb(msg);
                                }
                            }
                            Ok(msg @ ServerMessage::Dialogue { .. }) => {
                                debug!("Background: Dialogue received");
                                if let ServerMessage::Dialogue { ref message } = msg
                                    && let Some(ref cb) = dialogue_cb
                                {
                                    cb(message.clone());
                                }
                                if let Some(ref cb) = server_msg_cb {
                                    cb(msg);
                                }
                            }
                            Ok(msg @ ServerMessage::ImmediateEvent { .. }) => {
                                debug!("Background: ImmediateEvent received");
                                if let Some(ref cb) = server_msg_cb {
                                    cb(msg);
                                }
                            }
                            Ok(ServerMessage::ExecutionResult {
                                tick_id,
                                intent_id,
                                success,
                                error,
                                state_change_summary,
                            }) => {
                                debug!(
                                    "Background: ExecutionResult tick={}, intent={}, success={}",
                                    tick_id, intent_id, success
                                );
                                if let Some(ref tx) = exec_result_tx {
                                    let _ = tx.try_send(ExecutionResultData {
                                        tick_id,
                                        intent_id,
                                        success,
                                        error: error.clone(),
                                        state_change_summary: state_change_summary.clone(),
                                    });
                                }
                            }
                            Ok(ServerMessage::Error {
                                code,
                                message,
                                current_tick_id,
                            }) => {
                                let is_tick_mismatch =
                                    code == cyber_jianghu_protocol::ERROR_CODE_TICK_MISMATCH;

                                if let Some(ref cb) = server_msg_cb {
                                    cb(ServerMessage::Error {
                                        code: code.clone(),
                                        message: message.clone(),
                                        current_tick_id,
                                    });
                                }

                                if is_tick_mismatch {
                                    error!("Background: Tick mismatch: {}", message);
                                    // tick mismatch 自恢复：下一个 tick 的 WorldState 会自然到来
                                } else {
                                    warn!("Background: Server error: {}", message);
                                }
                            }
                            Ok(ServerMessage::Registered {
                                agent_id,
                                game_rules,
                                world_building_rules,
                                is_alive,
                                agent_name,
                                narrative_config,
                                narrative_config_hash,
                            }) => {
                                info!("Background: Registered agent_id={}, alive={}", agent_id, is_alive);
                                // 保存注册数据到 watch channel
                                if let Some(ref tx) = reg_tx {
                                    let _ = tx.send(Some(RegistrationData {
                                        agent_id,
                                        game_rules,
                                        world_building_rules,
                                        agent_name,
                                        is_alive,
                                        narrative_config,
                                        narrative_config_hash,
                                    }));
                                }
                            }
                            Ok(msg @ ServerMessage::AgentDied { .. }) => {
                                if let ServerMessage::AgentDied {
                                    agent_id,
                                    cause,
                                    description,
                                    ..
                                } = &msg
                                {
                                    let current_agent_id = {
                                        let guard = state.read().await;
                                        guard.agent_id
                                    };
                                    if current_agent_id == Some(*agent_id) {
                                        warn!("Agent {} died: {} - {}", agent_id, cause, description);
                                        if let Some(ref cb) = server_msg_cb {
                                            cb(msg.clone());
                                        }
                                    }
                                }
                            }
                            Ok(ServerMessage::Pong { .. }) => {
                                debug!("Background: Pong received");
                            }
                            Ok(ref msg @ ServerMessage::DailySummaryData { .. }) => {
                                debug!("Background: DailySummaryData received");
                                if let Some(ref cb) = server_msg_cb {
                                    cb(msg.clone());
                                }
                            }
                            Err(e) => {
                                warn!("Background: Parse error: {}", e);
                            }
                        }
                    }
                    Some(Ok(Message::Ping(_))) => {
                        // tungstenite 自动回复 Pong
                    }
                    Some(Ok(Message::Pong(_))) => {
                        debug!("Background: Pong received");
                    }
                    Some(Ok(Message::Close(_))) => {
                        warn!("Background: Server closed connection");
                        if let Some(ref tx) = {
                            let guard = state.read().await;
                            guard.worldstate_tx.clone()
                        } {
                            let _ = tx.send(None);
                        }
                        break;
                    }
                    Some(Err(e)) => {
                        error!("Background: WebSocket error: {}", e);
                        if let Some(ref tx) = {
                            let guard = state.read().await;
                            guard.worldstate_tx.clone()
                        } {
                            let _ = tx.send(None);
                        }
                        break;
                    }
                    None => {
                        warn!("Background: Stream ended");
                        if let Some(ref tx) = {
                            let guard = state.read().await;
                            guard.worldstate_tx.clone()
                        } {
                            let _ = tx.send(None);
                        }
                        break;
                    }
                    _ => {}
                }
            }
            // 发送 ClientMessage（Intent、SoulCycleReport 等统一通道）
            Some(msg) = intent_rx.recv() => {
                let json = match serde_json::to_string(&msg) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Background: Failed to serialize message: {}", e);
                        continue;
                    }
                };

                if let Err(e) = ws.send(Message::Text(json.into())).await {
                    error!("Background: Failed to send message: {}", e);
                    break;
                }
                debug!("Background: Sent message via unified channel");
            }
        }
    }

    info!("WebSocket background task exiting");
}

// ============================================================================
// 旧接口兼容（AgentClient）
// ============================================================================

/// Agent 客户端（兼容旧接口）
///
/// 使用 tokio::sync::RwLock 替代 std::sync::RwLock，避免跨 await 持有同步锁导致死锁
pub struct AgentClient {
    client: RwLock<WebSocketClient>,
}

impl AgentClient {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            client: RwLock::new(WebSocketClient::new(config)),
        }
    }

    /// 设置设备身份
    pub async fn set_identity(&self, device_id: Uuid, auth_token: String) {
        let mut client = self.client.write().await;
        client.set_identity(device_id, auth_token);
    }

    /// 更新服务器 URL（用于热切换）
    pub async fn update_server_url(&self, ws_url: String, http_url: String) {
        let mut client = self.client.write().await;
        client.update_server_url(ws_url, http_url);
    }

    /// 设置指定的 Agent ID（用于热切换）
    pub async fn set_agent_id(&self, agent_id: Option<Uuid>) {
        let client = self.client.read().await;
        client.set_agent_id(agent_id);
    }

    pub async fn connect(&self) -> Result<(), ConnectError> {
        let client = self.client.read().await;
        client.connect().await
    }

    pub async fn receive_world_state(&self) -> Result<WorldState> {
        let client = self.client.read().await;
        client.receive_world_state().await
    }

    pub async fn try_receive_execution_result(&self) -> Result<Vec<ExecutionResultData>> {
        let client = self.client.read().await;
        client.try_receive_execution_result().await
    }

    pub async fn wait_for_execution_result(
        &self,
        timeout_ms: u64,
    ) -> Result<Vec<ExecutionResultData>> {
        let client = self.client.read().await;
        client.wait_for_execution_result(timeout_ms).await
    }

    pub async fn send_intent(&self, intent: &Intent) -> Result<()> {
        let client = self.client.read().await;
        client.send_intent(intent).await
    }
    /// 获取 Intent 发送端
    pub async fn intent_sender(&self) -> Option<tokio::sync::mpsc::Sender<ClientMessage>> {
        let client = self.client.read().await;
        client.intent_sender().await
    }

    /// 发送三魂循环元数据
    pub async fn send_soul_cycle_report(
        &self,
        tick_id: i64,
        pipe_seq: i32,
        metadata: cyber_jianghu_protocol::SoulCycleMetadata,
    ) -> Result<()> {
        let client = self.client.read().await;
        client
            .send_soul_cycle_report(tick_id, pipe_seq, metadata)
            .await
    }

    /// 发送每日 LLM 日志摘要
    pub async fn send_daily_summary(&self, game_day: i64, summary: &str) -> Result<()> {
        let client = self.client.read().await;
        client.send_daily_summary(game_day, summary).await
    }

    pub async fn is_connected(&self) -> bool {
        let client = self.client.read().await;
        client.is_connected().await
    }

    /// 获取 Agent ID
    pub async fn agent_id(&self) -> Option<Uuid> {
        let client = self.client.read().await;
        client.agent_id()
    }

    /// 等待 Agent ID 可用（注册后）
    pub async fn wait_for_agent_id(&self) -> Result<Uuid> {
        let client = self.client.read().await;
        client.wait_for_agent_id().await
    }

    /// 设置游戏规则回调
    pub async fn set_game_rules_callback(&self, callback: Arc<dyn Fn(GameRules) + Send + Sync>) {
        let client = self.client.read().await;
        client.set_game_rules_callback(callback);
    }

    /// 设置对话消息回调
    pub async fn set_dialogue_callback(
        &self,
        callback: Arc<dyn Fn(DialogueMessage) + Send + Sync>,
    ) {
        let client = self.client.read().await;
        client.set_dialogue_callback(callback);
    }

    /// 设置世界观规则回调
    pub async fn set_world_building_rules_callback(
        &self,
        callback: Arc<dyn Fn(WorldBuildingRules) + Send + Sync>,
    ) {
        let client = self.client.read().await;
        client.set_world_building_rules_callback(callback);
    }

    /// 设置技能配置更新回调
    /// 参数: (skills, removed_items)
    pub async fn set_skill_update_callback(&self, callback: SkillUpdateCallback) {
        let client = self.client.read().await;
        client.set_skill_update_callback(callback);
    }

    /// 设置 Prompt 模板配置更新回调
    /// 参数: (PromptTemplateConfig)
    pub async fn set_prompt_template_callback(
        &self,
        callback: Arc<dyn Fn(cyber_jianghu_protocol::PromptTemplateConfig) + Send + Sync>,
    ) {
        let client = self.client.read().await;
        client.set_prompt_template_callback(callback);
    }

    /// 检查 WS 后台线程是否已成功投递 prompt_templates
    pub async fn is_prompt_template_received(&self) -> bool {
        self.client.read().await.is_prompt_template_received()
    }

    /// 设置事件特质规则更新回调（ConfigUpdate with config_type="persona_event_rules")
    pub async fn set_persona_event_rules_callback(
        &self,
        callback: Arc<dyn Fn(Vec<crate::component::persona::TraitMappingRule>) + Send + Sync>,
    ) {
        let client = self.client.read().await;
        client.set_persona_event_rules_callback(callback);
    }

    /// 设置叙事化配置更新回调（ConfigUpdate with config_type="narrative_config"）
    pub async fn set_narrative_config_callback(&self, callback: NarrativeConfigCallback) {
        let client = self.client.read().await;
        client.set_narrative_config_callback(callback);
    }

    /// 设置 Server 消息透传回调（用于 OpenClaw 集成）
    ///
    /// 当收到 Server 下行消息时，此回调会被调用，允许将消息
    /// 转发到外部系统（如 OpenClaw）
    pub async fn set_server_msg_callback(
        &self,
        callback: Arc<dyn Fn(ServerMessage) + Send + Sync>,
    ) {
        let client = self.client.read().await;
        client.set_server_msg_callback(callback);
    }

    /// 获取当前 server_msg_callback（用于 callback chaining）
    pub async fn get_server_msg_callback(
        &self,
    ) -> Option<Arc<dyn Fn(ServerMessage) + Send + Sync>> {
        let client = self.client.read().await;
        client.get_server_msg_callback()
    }

    /// 等待注册响应
    #[allow(clippy::type_complexity)]
    pub async fn wait_for_registration(
        &self,
    ) -> Result<
        Option<(
            Uuid,
            GameRules,
            Option<WorldBuildingRules>,
            Option<String>,
            bool,
            Option<cyber_jianghu_protocol::NarrativeConfig>,
            Option<String>,
        )>,
    > {
        let client = self.client.read().await;
        client.wait_for_registration().await
    }

    /// 获取游戏规则
    pub async fn game_rules(&self) -> Option<GameRules> {
        let client = self.client.read().await;
        client.game_rules()
    }

    /// 关闭连接
    pub async fn close(&self) {
        let client = self.client.read().await;
        client.disconnect().await
    }
}
