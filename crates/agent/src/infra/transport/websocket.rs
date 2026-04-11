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
    ClientMessage, DialogueMessage, GameRules, Intent, ServerMessage, WorldBuildingRules,
    WorldState,
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
}

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

    // ---- 后台任务架构 ----
    /// 后台 WebSocket 任务句柄
    reader_task: Option<tokio::task::JoinHandle<()>>,
    /// 关闭信号（broadcast，支持一次性触发）
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
    /// Intent 发送通道（主循环 → 后台任务）
    intent_tx: Option<tokio::sync::mpsc::Sender<Intent>>,
    /// 即时消息发送通道（立即响应 → 后台任务）
    immediate_msg_tx: Option<tokio::sync::mpsc::Sender<ClientMessage>>,
    /// WorldState 通道（后台任务 → 主循环，watch 保留最新值）
    worldstate_tx: Option<tokio::sync::watch::Sender<Option<WorldState>>>,
    /// 注册通知通道（后台任务 → 主循环）
    registered_tx: Option<tokio::sync::watch::Sender<Option<RegistrationData>>>,
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
                reader_task: None,
                shutdown_tx: None,
                intent_tx: None,
                immediate_msg_tx: None,
                worldstate_tx: None,
                registered_tx: None,
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

        let url_with_token = self.config.ws_url_with_token(*device_id, auth_token);
        let url =
            Url::parse(&url_with_token).map_err(|e| ConnectError::ConnectionFailed(e.into()))?;

        info!("Connecting to {}", self.config.ws_url);

        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((ws, _)) => {
                let mut state = self.state.write().await;

                // 创建通道
                let (intent_tx, intent_rx) = tokio::sync::mpsc::channel(32);
                let (immediate_msg_tx, immediate_msg_rx) = tokio::sync::mpsc::channel(32);
                let (worldstate_tx, _) = tokio::sync::watch::channel(None);
                let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);
                let (registered_tx, _) = tokio::sync::watch::channel(None);

                // 启动后台 WebSocket 任务（独占 ws）
                let state_arc = self.state.clone();
                let handle = tokio::spawn(async move {
                    websocket_background_task(
                        ws,
                        state_arc,
                        intent_rx,
                        immediate_msg_rx,
                        shutdown_rx,
                    )
                    .await;
                });

                // 更新状态
                state.connected = true;
                state.agent_id = None;
                state.game_rules = None;
                state.world_building_rules = None;
                state.reader_task = Some(handle);
                state.shutdown_tx = Some(shutdown_tx);
                state.intent_tx = Some(intent_tx);
                state.immediate_msg_tx = Some(immediate_msg_tx);
                state.worldstate_tx = Some(worldstate_tx);
                state.registered_tx = Some(registered_tx);

                info!("Connected to server (background task started)");
                Ok(())
            }
            Err(tokio_tungstenite::tungstenite::Error::Http(resp))
                if resp.status().as_u16() == 400 =>
            {
                warn!("WebSocket auth failed (HTTP 400)");
                Err(ConnectError::AuthFailed)
            }
            Err(e) => Err(ConnectError::ConnectionFailed(anyhow::anyhow!(
                "Failed to connect to WebSocket server: {}",
                e
            ))),
        }
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

    /// 等待注册响应
    ///
    /// 返回值：
    /// - `Ok(Some((agent_id, game_rules, agent_name, is_alive)))` - 有角色（活或死）
    /// - `Ok(None)` - 无角色，等待注册（agent_id 为 nil）
    /// - `Err(e)` - 连接错误
    pub async fn wait_for_registration(
        &self,
    ) -> Result<Option<(Uuid, GameRules, Option<String>, bool)>> {
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
                        data.agent_name.clone(),
                        data.is_alive,
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

        tx.send(intent.clone())
            .await
            .context("Failed to send intent to background task")?;

        debug!("Sent Intent to background: {:?}", intent.action_type);
        Ok(())
    }

    /// 发送即时消息（speak/whisper 等，通过独立 channel，不阻塞主 intent）
    pub async fn send_immediate_message(&self, msg: ClientMessage) -> Result<()> {
        let tx = {
            let state = self.state.read().await;
            state
                .immediate_msg_tx
                .as_ref()
                .context("Not connected to server")?
                .clone()
        };

        tx.send(msg)
            .await
            .context("Failed to send immediate message to background task")?;

        debug!("Sent immediate message to background task");
        Ok(())
    }

    /// 发送三魂循环元数据到服务器
    ///
    /// 在 intent 发送后调用，使 server-web 能看到与 agent-web 相同的三魂详情。
    /// 使用即时消息通道（fire-and-forget，不阻塞主循环）。
    pub async fn send_soul_cycle_report(
        &self,
        tick_id: i64,
        metadata: cyber_jianghu_protocol::SoulCycleMetadata,
    ) -> Result<()> {
        let agent_id = self.agent_id();
        let msg = ClientMessage::SoulCycleReport {
            tick_id,
            agent_id,
            metadata,
        };
        self.send_immediate_message(msg).await
    }

    /// 获取即时消息发送端的 clone（用于绑定到 ImmediateEventHandler）
    pub async fn immediate_msg_sender(&self) -> Option<tokio::sync::mpsc::Sender<ClientMessage>> {
        let state = self.state.read().await;
        state.immediate_msg_tx.clone()
    }

    /// 获取游戏规则
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
            state.immediate_msg_tx = None;
            state.worldstate_tx = None;
            state.registered_tx = None;

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
    mut intent_rx: tokio::sync::mpsc::Receiver<Intent>,
    mut immediate_msg_rx: tokio::sync::mpsc::Receiver<ClientMessage>,
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
                        let (game_rules_cb, dialogue_cb, wb_rules_cb, action_update_cb, server_msg_cb, ws_tx, reg_tx) = {
                            let state_guard = state.read().await;
                            (
                                state_guard.game_rules_callback.clone(),
                                state_guard.dialogue_callback.clone(),
                                state_guard.world_building_rules_callback.clone(),
                                state_guard.action_update_callback.clone(),
                                state_guard.server_msg_callback.clone(),
                                state_guard.worldstate_tx.clone(),
                                state_guard.registered_tx.clone(),
                            )
                        };

                        match serde_json::from_str::<ServerMessage>(&text) {
                            Ok(ServerMessage::WorldState { data }) => {
                                debug!("Background: WorldState tick={}", data.tick_id);
                                if let Some(ref tx) = ws_tx {
                                    let _ = tx.send(Some(data));
                                }
                            }
                            Ok(msg @ ServerMessage::GameRulesUpdate { .. }) => {
                                if let ServerMessage::GameRulesUpdate { ref game_rules } = msg {
                                    info!("Background: GameRules v{}", game_rules.version);
                                    {
                                        let mut guard = state.write().await;
                                        guard.game_rules = Some(game_rules.clone());
                                    }
                                    if let Some(ref cb) = game_rules_cb {
                                        cb(game_rules.clone());
                                    }
                                }
                                if let Some(ref cb) = server_msg_cb {
                                    cb(msg);
                                }
                            }
                            Ok(msg @ ServerMessage::WorldBuildingRulesUpdate { .. }) => {
                                if let ServerMessage::WorldBuildingRulesUpdate { ref rules } = msg {
                                    info!("Background: WorldBuildingRules v{}", rules.version);
                                    {
                                        let mut guard = state.write().await;
                                        guard.world_building_rules = Some(rules.clone());
                                    }
                                    if let Some(ref cb) = wb_rules_cb {
                                        cb(rules.clone());
                                    }
                                }
                                if let Some(ref cb) = server_msg_cb {
                                    cb(msg);
                                }
                            }
                            Ok(msg @ ServerMessage::ActionUpdate { .. }) => {
                                if let ServerMessage::ActionUpdate {
                                    ref update_type,
                                    ref version,
                                    ref updated_actions,
                                    ref removed_actions,
                                    ..
                                } = msg
                                {
                                    info!(
                                        "Background: ActionUpdate type={}, v={}, +{}, -{}",
                                        update_type, version,
                                        updated_actions.len(), removed_actions.len()
                                    );
                                    if let Some(ref cb) = action_update_cb {
                                        cb(msg.clone());
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
                                    }));
                                }
                            }
                            Ok(msg @ ServerMessage::AgentDied { .. }) => {
                                if let ServerMessage::AgentDied {
                                    agent_id,
                                    ref cause,
                                    ref description,
                                    ..
                                } = msg
                                {
                                    let current_agent_id = {
                                        let guard = state.read().await;
                                        guard.agent_id
                                    };
                                    if current_agent_id == Some(agent_id) {
                                        warn!("Agent {} died: {} - {}", agent_id, cause, description);
                                        if let Some(ref cb) = server_msg_cb {
                                            cb(msg);
                                        }
                                    }
                                }
                            }
                            Ok(ServerMessage::Pong { .. }) => {
                                debug!("Background: Pong received");
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

            // 发送 intent（通过 mpsc channel）
            Some(intent) = intent_rx.recv() => {
                let client_msg = ClientMessage::from_intent(intent.clone());
                let json = match serde_json::to_string(&client_msg) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Background: Failed to serialize intent: {}", e);
                        continue;
                    }
                };

                if let Err(e) = ws.send(Message::Text(json.into())).await {
                    error!("Background: Failed to send intent: {}", e);
                    break;
                }
                debug!("Background: Sent intent action={}", intent.action_type);
            }

            // 发送即时消息（ImmediateIntent，通过单独的 channel）
            Some(immediate_msg) = immediate_msg_rx.recv() => {
                let json = match serde_json::to_string(&immediate_msg) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Background: Failed to serialize immediate message: {}", e);
                        continue;
                    }
                };

                if let Err(e) = ws.send(Message::Text(json.into())).await {
                    error!("Background: Failed to send immediate message: {}", e);
                    break;
                }
                debug!("Background: Sent immediate message");
            }
        }
    }

    // 清理连接状态：drop worldstate_tx 使所有 receiver 收到 Closed 错误
    {
        let mut guard = state.write().await;
        guard.connected = false;
        guard.worldstate_tx = None;
        guard.intent_tx = None;
        guard.immediate_msg_tx = None;
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

    pub async fn connect(&self) -> Result<(), ConnectError> {
        let client = self.client.read().await;
        client.connect().await
    }

    pub async fn receive_world_state(&self) -> Result<WorldState> {
        let client = self.client.read().await;
        client.receive_world_state().await
    }

    pub async fn send_intent(&self, intent: &Intent) -> Result<()> {
        let client = self.client.read().await;
        client.send_intent(intent).await
    }

    /// 发送即时消息（speak/whisper 等）
    pub async fn send_immediate_message(&self, msg: ClientMessage) -> Result<()> {
        let client = self.client.read().await;
        client.send_immediate_message(msg).await
    }

    /// 发送三魂循环元数据到服务器
    pub async fn send_soul_cycle_report(
        &self,
        tick_id: i64,
        metadata: cyber_jianghu_protocol::SoulCycleMetadata,
    ) -> Result<()> {
        let client = self.client.read().await;
        client.send_soul_cycle_report(tick_id, metadata).await
    }

    /// 获取即时消息发送端
    pub async fn immediate_msg_sender(&self) -> Option<tokio::sync::mpsc::Sender<ClientMessage>> {
        let client = self.client.read().await;
        client.immediate_msg_sender().await
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
    ///
    /// 返回值：
    /// - `Ok(Some((agent_id, game_rules, agent_name, is_alive)))` - 有角色（活或死）
    /// - `Ok(None)` - 无角色，等待注册（agent_id 为 nil）
    /// - `Err(e)` - 连接错误
    pub async fn wait_for_registration(
        &self,
    ) -> Result<Option<(Uuid, GameRules, Option<String>, bool)>> {
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
