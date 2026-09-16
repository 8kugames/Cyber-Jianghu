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
    ClientMessage, ConfigType, DialogueMessage, GameRules, Intent, ServerMessage, SkillContent,
    WorldBuildingRules, WorldEvent, WorldState,
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
    pub governance_code: Option<cyber_jianghu_protocol::GovernanceCode>,
}

/// 技能配置更新回调类型
pub type SkillUpdateCallback = Arc<dyn Fn(Vec<SkillContent>, Vec<String>) + Send + Sync>;

/// 叙事化配置更新回调类型
type NarrativeConfigCallback =
    Arc<dyn Fn(cyber_jianghu_protocol::NarrativeConfig, Option<String>) + Send + Sync>;

/// 连接状态
#[derive(Default)]
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
    /// 事件流通道（后台任务 → 主循环）。watch 只保最新 WorldState（快照语义，
    /// latest-wins）；events_log 是 append-only 流，随快照覆盖会永久丢失
    /// （死亡等不可重复事件），故单独走有界队列，由主循环 drain 后合并。
    events_tx: Option<tokio::sync::mpsc::Sender<Vec<WorldEvent>>>,
    /// 事件流接收端（Arc<Mutex> 允许 &self 下异步访问）
    events_rx:
        Option<std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Vec<WorldEvent>>>>>,
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
                events_tx: None,
                events_rx: None,
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
                let (events_tx, events_rx) = tokio::sync::mpsc::channel(64);

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
                state.events_tx = Some(events_tx);
                state.events_rx = Some(std::sync::Arc::new(tokio::sync::Mutex::new(events_rx)));

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

    /// 设置动作配置更新回调（ConfigUpdate with config_type="actions"）
    ///
    /// 参数为完整 ServerMessage（content 为 Vec<AvailableAction> 的 JSON）。
    /// 修复：该回调此前从未接线，server 的动作配置推送全部落空，
    /// 部署/热更新后 Agent 词表漂移无法自愈。
    pub fn set_action_update_callback(&self, callback: Arc<dyn Fn(ServerMessage) + Send + Sync>) {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let mut state = self.state.write().await;
                state.action_update_callback = Some(callback);
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

    /// 非阻塞 drain 事件流队列（被覆盖 WorldState 快照中的 events_log 在此找回）。
    ///
    /// 与 receive_world_state 配套：watch 通道只保留最新快照，主循环每轮在
    /// 消费最新 WorldState 后调用本方法，把期间被覆盖快照的事件合并回当轮处理。
    pub async fn try_drain_pending_events(&self) -> Vec<WorldEvent> {
        let rx_arc = {
            let state = self.state.read().await;
            state.events_rx.clone()
        };
        let Some(rx) = rx_arc else {
            return Vec::new();
        };
        let mut guard = rx.lock().await;
        let mut out = Vec::new();
        while let Ok(batch) = guard.try_recv() {
            out.extend(batch);
        }
        out
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
        match recv_with_timeout(&mut rx, std::time::Duration::from_millis(timeout_ms)).await {
            Ok(Some(first)) => {
                results.push(first);
                // drain 后续结果（server 连续发送，应该已在缓冲区）
                while let Ok(data) = rx.try_recv() {
                    results.push(data);
                }
                Ok(results)
            }
            Ok(None) => unreachable!("recv_with_timeout 不返回 Ok(None)"),
            Err(e) => Err(e),
        }
    }

    /// 发送 Intent（通过 mpsc channel → 后台任务）
    pub async fn send_intent(
        &self,
        intent: &Intent,
        soul_cycle_metadata: Option<cyber_jianghu_protocol::SoulCycleMetadata>,
    ) -> Result<()> {
        let tx = {
            let state = self.state.read().await;
            state
                .intent_tx
                .as_ref()
                .context("Not connected to server")?
                .clone()
        };

        tx.send(ClientMessage::from_intent_with_extras(
            intent.clone(),
            soul_cycle_metadata,
        ))
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

    /// 发送关系图谱全量快照到服务器
    ///
    /// 游戏日结束时随 DailySummary 一起发送，server 全量覆盖（DELETE+INSERT）。
    /// 关系列表来自 `RelationshipStore::get_all_relationships()`，由 caller 完成向
    /// protocol 类型（i64 毫秒时间戳）的转换后再传入。
    pub async fn send_relationship_snapshot(
        &self,
        agent_id: uuid::Uuid,
        game_day: i64,
        relationships: Vec<cyber_jianghu_protocol::types::RelationshipMemory>,
    ) -> Result<()> {
        let msg = ClientMessage::RelationshipSnapshot {
            agent_id,
            game_day,
            relationships,
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
            .context("Failed to send relationship snapshot")?;
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
            if let Some(tx) = state.shutdown_tx.take()
                && let Err(e) = tx.send(())
            {
                tracing::warn!("shutdown_tx.send 失败（receiver 可能已 drop）：{e:?}");
            }

            let handle = state.reader_task.take();
            state.connected = false;
            state.intent_tx = None;
            state.worldstate_tx = None;
            state.registered_tx = None;
            state.execution_result_tx = None;
            state.execution_result_rx = None;
            state.events_tx = None;
            state.events_rx = None;

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

mod agent_client;
mod background;
mod helpers;

pub use agent_client::AgentClient;
use background::websocket_background_task;
use helpers::{handle_worldstate_send_failure, recv_with_timeout};

#[cfg(test)]
#[path = "websocket_tests.rs"]
mod tests;
