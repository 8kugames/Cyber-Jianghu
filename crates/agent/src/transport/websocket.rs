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
    state: Arc<RwLock<ConnectionState>>,
}

/// 连接状态
struct ConnectionState {
    ws: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    connected: bool,
    /// Agent ID（注册后设置）
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
}

impl WebSocketClient {
    /// 创建新的客户端
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(ConnectionState {
                ws: None,
                connected: false,
                agent_id: None,
                game_rules: None,
                world_building_rules: None,
                game_rules_callback: None,
                dialogue_callback: None,
                world_building_rules_callback: None,
            })),
        }
    }

    /// 连接到服务器
    pub async fn connect(&self) -> Result<()> {
        let url = Url::parse(&self.config.ws_url)
            .context("Invalid WebSocket URL")?;

        info!("Connecting to {}", url);

        let (ws, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .context("Failed to connect to WebSocket server")?;

        let mut state = self.state.write().await;
        state.ws = Some(ws);
        state.connected = true;

        info!("Connected to server");
        Ok(())
    }

    /// 获取 Agent ID
    pub fn agent_id(&self) -> Option<Uuid> {
        // 使用 try_read 避免异步
        self.state.try_read().ok()?.agent_id
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
    pub fn set_world_building_rules_callback(&self, callback: Arc<dyn Fn(WorldBuildingRules) + Send + Sync>) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.world_building_rules_callback = Some(callback);
            });
        });
    }

    /// 等待注册响应
    pub async fn wait_for_registration(&self) -> Result<(Uuid, GameRules)> {
        let mut state = self.state.write().await;

        let ws = state.ws.as_mut()
            .context("Not connected to server")?;

        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    // 尝试解析为 ServerMessage
                    match serde_json::from_str::<ServerMessage>(&text) {
                        Ok(ServerMessage::Registered { agent_id, game_rules, world_building_rules }) => {
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
                    return Err(anyhow::anyhow!("Server closed connection during registration"));
                }
                Some(Err(e)) => {
                    error!("WebSocket error during registration: {}", e);
                    state.connected = false;
                    state.ws = None;
                    return Err(anyhow::anyhow!("WebSocket error during registration: {}", e));
                }
                None => {
                    warn!("WebSocket stream ended during registration");
                    state.connected = false;
                    state.ws = None;
                    return Err(anyhow::anyhow!("WebSocket stream ended during registration"));
                }
                _ => {}
            }
        }
    }

    /// 接收消息并处理
    async fn receive_and_handle_message(&self) -> Result<Option<WorldState>> {
        // 先获取回调的克隆，避免在循环中同时持有可变和不可变借用
        let (game_rules_cb, dialogue_cb, world_building_rules_cb) = {
            let state = self.state.read().await;
            (
                state.game_rules_callback.clone(),
                state.dialogue_callback.clone(),
                state.world_building_rules_callback.clone(),
            )
        };

        loop {
            // 每次迭代获取消息，然后释放锁
            let message_result = {
                let mut state = self.state.write().await;
                let ws = state.ws.as_mut()
                    .context("Not connected to server")?;
                ws.next().await
            };

            match message_result {
                Some(Ok(Message::Text(text))) => {
                    // 尝试解析为 ServerMessage
                    match serde_json::from_str::<ServerMessage>(&text) {
                        Ok(ServerMessage::WorldState { data }) => {
                            debug!("Received WorldState: tick_id={}", data.tick_id);
                            return Ok(Some(data));
                        }
                        Ok(ServerMessage::GameRulesUpdate { game_rules }) => {
                            info!("Received game rules update: version {}", game_rules.version);
                            // 更新状态
                            {
                                let mut state = self.state.write().await;
                                state.game_rules = Some(game_rules.clone());
                            }
                            // 使用之前克隆的回调
                            if let Some(ref callback) = game_rules_cb {
                                callback(game_rules);
                            }
                            // 继续等待 WorldState
                        }
                        Ok(ServerMessage::WorldBuildingRulesUpdate { rules }) => {
                            info!("Received world building rules update: version {}", rules.version);
                            // 更新状态
                            {
                                let mut state = self.state.write().await;
                                state.world_building_rules = Some(rules.clone());
                            }
                            // 使用之前克隆的回调
                            if let Some(ref callback) = world_building_rules_cb {
                                callback(rules);
                            }
                            // 继续等待 WorldState
                        }
                        Ok(ServerMessage::Dialogue { message }) => {
                            debug!("Received dialogue message");
                            // 使用之前克隆的回调
                            if let Some(ref callback) = dialogue_cb {
                                callback(message);
                            }
                            // 继续等待 WorldState
                        }
                        Ok(ServerMessage::Pong { .. }) => {
                            // 心跳响应，忽略
                        }
                        Ok(ServerMessage::Error { message }) => {
                            warn!("Server error: {}", message);
                            // 继续等待
                        }
                        Ok(ServerMessage::Registered { .. }) => {
                            // 已经注册过了，忽略
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

        let ws = state.ws.as_mut()
            .context("Not connected to server")?;

        let client_msg = ClientMessage::from_intent(intent.clone());
        let json = serde_json::to_string(&client_msg)
            .context("Failed to serialize Intent")?;

        ws.send(Message::Text(json.into())).await
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
pub struct AgentClient {
    client: WebSocketClient,
}

impl AgentClient {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            client: WebSocketClient::new(config),
        }
    }

    pub async fn connect(&self) -> Result<()> {
        self.client.connect().await
    }

    pub async fn receive_world_state(&self) -> Result<WorldState> {
        self.client.receive_world_state().await
    }

    pub async fn send_intent(&self, intent: &Intent) -> Result<()> {
        self.client.send_intent(intent).await
    }

    pub async fn is_connected(&self) -> bool {
        self.client.is_connected().await
    }

    /// 获取 Agent ID
    pub fn agent_id(&self) -> Option<Uuid> {
        self.client.agent_id()
    }

    /// 设置游戏规则回调
    pub fn set_game_rules_callback(&self, callback: Arc<dyn Fn(GameRules) + Send + Sync>) {
        self.client.set_game_rules_callback(callback);
    }

    /// 设置对话消息回调
    pub fn set_dialogue_callback(&self, callback: Arc<dyn Fn(DialogueMessage) + Send + Sync>) {
        self.client.set_dialogue_callback(callback);
    }

    /// 设置世界观规则回调
    pub fn set_world_building_rules_callback(&self, callback: Arc<dyn Fn(WorldBuildingRules) + Send + Sync>) {
        self.client.set_world_building_rules_callback(callback);
    }

    /// 等待注册响应
    pub async fn wait_for_registration(&self) -> Result<(Uuid, GameRules)> {
        self.client.wait_for_registration().await
    }

    /// 获取游戏规则
    pub fn game_rules(&self) -> Option<GameRules> {
        self.client.game_rules()
    }

    /// 关闭连接
    pub async fn close(&self) {
        self.client.disconnect().await
    }
}
