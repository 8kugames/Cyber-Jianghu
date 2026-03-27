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

use anyhow::{Context, Result};
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
// WebSocket 客户端
// ============================================================================

/// WebSocket 客户端（纯 I/O）
pub struct WebSocketClient {
    config: ServerConfig,
    /// 设备身份（device_id + auth_token）
    identity: Option<(Uuid, String)>,
    state: Arc<RwLock<ConnectionState>>,
}

/// 连接状态
struct ConnectionState {
    ws: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
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
}

impl WebSocketClient {
    /// 创建新的客户端
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            identity: None,
            state: Arc::new(RwLock::new(ConnectionState {
                ws: None,
                connected: false,
                agent_id: None,
                game_rules: None,
                world_building_rules: None,
                game_rules_callback: None,
                dialogue_callback: None,
                world_building_rules_callback: None,
                server_msg_callback: None,
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
    pub async fn connect(&self) -> Result<()> {
        let (device_id, auth_token) = self
            .identity
            .as_ref()
            .context("Identity not set. Call set_identity() first.")?;

        let url_with_token = self.config.ws_url_with_token(*device_id, auth_token);
        let url = Url::parse(&url_with_token).context("Invalid WebSocket URL")?;

        info!("Connecting to {}", self.config.ws_url);

        let (ws, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .context("Failed to connect to WebSocket server")?;

        let mut state = self.state.write().await;
        state.ws = Some(ws);
        state.connected = true;
        state.agent_id = None;
        state.game_rules = None;
        state.world_building_rules = None;

        info!("Connected to server");
        Ok(())
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

    /// 等待注册响应
    pub async fn wait_for_registration(&self) -> Result<(Uuid, GameRules)> {
        let mut state = self.state.write().await;

        let ws = state.ws.as_mut().context("Not connected to server")?;

        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    // 尝试解析为 ServerMessage
                    match serde_json::from_str::<ServerMessage>(&text) {
                        Ok(ServerMessage::Registered {
                            agent_id,
                            game_rules,
                            world_building_rules,
                        }) => {
                            // agent_id 为零 = 需要注册新角色
                            if agent_id == Uuid::nil() {
                                return Err(anyhow::anyhow!(
                                    "Pending registration: no active character, please register"
                                ));
                            }
                            state.agent_id = Some(agent_id);
                            state.game_rules = Some(game_rules.clone());
                            if let Some(rules) = world_building_rules {
                                state.world_building_rules = Some(rules);
                            }
                            info!("Agent registered with ID: {}", agent_id);
                            return Ok((agent_id, game_rules));
                        }
                        Ok(ServerMessage::Error { message }) => {
                            return Err(anyhow::anyhow!("Server error: {}", message));
                        }
                        _ => {
                            // 其他消息，忽略，继续等待
                            debug!("Received non-registration message, waiting for registration");
                        }
                    }
                }
                Some(Ok(Message::Ping(_))) => {
                    // 心跳，忽略
                }
                Some(Ok(Message::Pong(_))) => {
                    // 心跳响应，忽略
                }
                Some(Ok(Message::Close(_))) => {
                    warn!("Server closed connection during registration");
                    state.connected = false;
                    state.ws = None;
                    return Err(anyhow::anyhow!(
                        "Server closed connection during registration"
                    ));
                }
                Some(Err(e)) => {
                    error!("WebSocket error during registration: {}", e);
                    state.connected = false;
                    state.ws = None;
                    return Err(anyhow::anyhow!(
                        "WebSocket error during registration: {}",
                        e
                    ));
                }
                None => {
                    warn!("WebSocket stream ended during registration");
                    state.connected = false;
                    state.ws = None;
                    return Err(anyhow::anyhow!(
                        "WebSocket stream ended during registration"
                    ));
                }
                _ => {}
            }
        }
    }

    /// 接收消息并处理
    async fn receive_and_handle_message(&self) -> Result<Option<WorldState>> {
        // 先获取回调的克隆，避免在循环中同时持有可变和不可变借用
        let (game_rules_cb, dialogue_cb, world_building_rules_cb, server_msg_cb) = {
            let state = self.state.read().await;
            (
                state.game_rules_callback.clone(),
                state.dialogue_callback.clone(),
                state.world_building_rules_callback.clone(),
                state.server_msg_callback.clone(),
            )
        };

        loop {
            // 每次迭代获取消息，然后释放锁
            let message_result = {
                let mut state = self.state.write().await;
                let ws = state.ws.as_mut().context("Not connected to server")?;
                ws.next().await
            };

            match message_result {
                Some(Ok(Message::Text(text))) => {
                    // 尝试解析为 ServerMessage
                    match serde_json::from_str::<ServerMessage>(&text) {
                        Ok(ServerMessage::WorldState { data }) => {
                            debug!("Received WorldState: tick_id={}", data.tick_id);
                            // WorldState 不透传（已有专门的 Tick 处理）
                            return Ok(Some(data));
                        }
                        Ok(msg @ ServerMessage::GameRulesUpdate { .. }) => {
                            if let ServerMessage::GameRulesUpdate { ref game_rules } = msg {
                                info!("Received game rules update: version {}", game_rules.version);
                                // 更新状态
                                {
                                    let mut state = self.state.write().await;
                                    state.game_rules = Some(game_rules.clone());
                                }
                                // 使用之前克隆的回调
                                if let Some(ref callback) = game_rules_cb {
                                    callback(game_rules.clone());
                                }
                            }
                            // 透传给 OpenClaw
                            if let Some(ref callback) = server_msg_cb {
                                callback(msg);
                            }
                            // 继续等待 WorldState
                        }
                        Ok(msg @ ServerMessage::WorldBuildingRulesUpdate { .. }) => {
                            if let ServerMessage::WorldBuildingRulesUpdate { ref rules } = msg {
                                info!(
                                    "Received world building rules update: version {}",
                                    rules.version
                                );
                                // 更新状态
                                {
                                    let mut state = self.state.write().await;
                                    state.world_building_rules = Some(rules.clone());
                                }
                                // 使用之前克隆的回调
                                if let Some(ref callback) = world_building_rules_cb {
                                    callback(rules.clone());
                                }
                            }
                            // 透传给 OpenClaw
                            if let Some(ref callback) = server_msg_cb {
                                callback(msg);
                            }
                            // 继续等待 WorldState
                        }
                        Ok(msg @ ServerMessage::Dialogue { .. }) => {
                            debug!("Received dialogue message");
                            // 使用之前克隆的回调
                            if let ServerMessage::Dialogue { ref message } = msg
                                && let Some(ref callback) = dialogue_cb
                            {
                                callback(message.clone());
                            }
                            // 透传给 OpenClaw
                            if let Some(ref callback) = server_msg_cb {
                                callback(msg);
                            }
                            // 继续等待 WorldState
                        }
                        Ok(msg @ ServerMessage::Error { .. }) => {
                            if let ServerMessage::Error { ref message } = msg {
                                warn!("Server error: {}", message);
                            }
                            // 透传给 OpenClaw（错误消息很重要）
                            if let Some(ref callback) = server_msg_cb {
                                callback(msg);
                            }
                            // 继续等待
                        }
                        Ok(msg @ ServerMessage::AgentDied { .. }) => {
                            if let ServerMessage::AgentDied {
                                agent_id,
                                ref cause,
                                ref description,
                                ..
                            } = msg
                            {
                                warn!("Agent {} died: {} - {}", agent_id, cause, description);
                            }
                            // 透传给 OpenClaw（触发重生流程）
                            if let Some(ref callback) = server_msg_cb {
                                callback(msg);
                            }
                            // 继续等待
                        }
                        Ok(ServerMessage::Pong { .. }) => {
                            // 心跳响应，不透传
                        }
                        Ok(ServerMessage::Registered { .. }) => {
                            // 已经注册过了，不透传
                            debug!("Received duplicate registration message");
                        }
                        Err(e) => {
                            warn!("Failed to parse server message: {}", e);
                            // 继续等待
                        }
                    }
                }
                Some(Ok(Message::Ping(_))) => {
                    // 心跳，忽略
                }
                Some(Ok(Message::Pong(_))) => {
                    // 心跳响应，忽略
                }
                Some(Ok(Message::Close(_))) => {
                    warn!("Server closed connection");
                    let mut state = self.state.write().await;
                    state.connected = false;
                    state.ws = None;
                    return Err(anyhow::anyhow!("Server closed connection"));
                }
                Some(Err(e)) => {
                    error!("WebSocket error: {}", e);
                    let mut state = self.state.write().await;
                    state.connected = false;
                    state.ws = None;
                    return Err(anyhow::anyhow!("WebSocket error: {}", e));
                }
                None => {
                    warn!("WebSocket stream ended");
                    let mut state = self.state.write().await;
                    state.connected = false;
                    state.ws = None;
                    return Err(anyhow::anyhow!("WebSocket stream ended"));
                }
                _ => {}
            }
        }
    }

    /// 接收 WorldState（阻塞直到收到）
    pub async fn receive_world_state(&self) -> Result<WorldState> {
        loop {
            match self.receive_and_handle_message().await? {
                Some(world_state) => return Ok(world_state),
                None => continue,
            }
        }
    }

    /// 发送 Intent
    pub async fn send_intent(&self, intent: &Intent) -> Result<()> {
        let mut state = self.state.write().await;

        let ws = state.ws.as_mut().context("Not connected to server")?;

        let client_msg = ClientMessage::from_intent(intent.clone());
        let json = serde_json::to_string(&client_msg).context("Failed to serialize Intent")?;

        ws.send(Message::Text(json.into()))
            .await
            .context("Failed to send Intent")?;

        debug!("Sent Intent: {:?}", intent.action_type);
        Ok(())
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
        let mut state = self.state.write().await;
        if let Some(mut ws) = state.ws.take() {
            let _ = ws.close(None).await;
        }
        state.connected = false;
        info!("Disconnected from server");
    }
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

    pub async fn connect(&self) -> Result<()> {
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

    /// 等待注册响应
    pub async fn wait_for_registration(&self) -> Result<(Uuid, GameRules)> {
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
