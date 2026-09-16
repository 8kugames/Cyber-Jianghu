//! HTTP API 状态类型（HttpApiState / HttpDecisionConfig / IntentRequest 等）

use super::*;

/// HTTP API 配置
///
/// 配置 HTTP API 服务器的运行参数（辅助功能）
pub struct HttpDecisionConfig {
    /// 监听端口
    pub port: u16,
    /// 决策超时（秒），超过此时间返回 idle 意图
    /// tick_duration=60s，留 5s 余量用于 intent 提交和网络往返
    pub timeout_secs: u64,
}

impl Default for HttpDecisionConfig {
    fn default() -> Self {
        Self {
            port: 0,          // 0 = 随机端口
            timeout_secs: 55, // tick=60s − 5s 余量
        }
    }
}

/// HTTP API 共享状态
///
/// 包含所有需要通过 API 访问的组件和状态
/// 所有 AI 组件都是可选的，初始化失败仍可运行基础功能
#[derive(Clone)]
pub struct HttpApiState {
    /// 当前游戏世界状态
    pub current_state: Arc<RwLock<Option<WorldState>>>,
    /// 状态最后更新时间
    pub last_state_update: Arc<RwLock<Option<std::time::Instant>>>,
    /// Intent 发送通道，将外部提交的 Intent 发送给决策函数
    pub intent_tx: mpsc::Sender<Intent>,
    /// 当前 Agent ID (共享，WebSocket 注册后会更新)
    pub agent_id: Arc<RwLock<Uuid>>,

    // === Tick 时序信息 ===
    /// Tick 持续时间（秒），从 GameRules 获取
    pub tick_duration_secs: Arc<std::sync::atomic::AtomicU64>,

    // === 服务器连接配置 ===
    /// Server HTTP URL（用于角色注册等 API 调用）
    /// 使用 RwLock 支持运行时热重载
    pub server_http_url: Arc<RwLock<String>>,
    /// Server WebSocket URL（用于实时通信）
    pub server_ws_url: Arc<RwLock<String>>,
    /// 设备配置（device_id + auth_token），运行时可通过注册更新
    pub device_config: Arc<RwLock<Option<crate::config::DeviceConfig>>>,
    /// 服务器配置目录路径（运行时可通过服务器切换更新）
    pub server_dir: Arc<RwLock<PathBuf>>,
    /// 角色配置目录路径（运行时可通过服务器切换更新）
    pub character_dir: Arc<RwLock<PathBuf>>,
    /// 配置文件路径（用于读取角色配置）
    pub config_path: PathBuf,

    // AI 组件（全部可选，支持按需注入）
    /// 对话客户端，处理 Agent 间对话
    pub dialogue_client: Option<Arc<DialogueClient>>,
    /// 关系存储，持久化存储与其他 Agent 的关系记忆
    pub relationship_store: Arc<std::sync::RwLock<Option<Arc<RelationshipStore>>>>,
    /// 寿命计算器，计算年龄和老化效果
    /// 记忆管理器，管理工作记忆、情景记忆和语义记忆
    /// 与 Agent 共享同一 Arc<RwLock<MemoryManager>> 实例
    pub memory_manager: Arc<tokio::sync::RwLock<Option<Arc<tokio::sync::RwLock<MemoryManager>>>>>,
    /// 记忆管理器基础配置模板（用于热切角色）
    pub memory_config_template: Option<crate::component::memory::MemoryManagerConfig>,
    /// 统一意图审查器，供 HTTP validate 与 Claw WS 共用
    pub intent_validator: Option<Arc<dyn Validator>>,
    /// 最近一份 GameRules（用于构造分级审查上下文）
    pub game_rules: Arc<RwLock<Option<cyber_jianghu_protocol::GameRules>>>,
    /// 叙事生成器（可选，仅在有 LlmClient 时可用）
    pub narrative_generator: Option<Arc<NarrativeGenerator>>,
    /// 动态人设（可选；persona 在 api_state 之后创建，事后经 set_dynamic_persona 注入）
    pub dynamic_persona: std::sync::Arc<std::sync::RwLock<Option<ThreadSafePersona>>>,
    /// 三魂循环记录器注册表，按 agent_id 隔离
    /// 支持多角色：当前角色写入 + 所有角色读取
    pub soul_cycle_registrar:
        Arc<RwLock<HashMap<Uuid, Arc<soul_cycle_recorder::SoulCycleRecorder>>>>,
    /// 数据目录路径（用于按需加载角色的 SQLite 文件）
    pub data_dir: PathBuf,
    /// 托梦存储，管理持续 n 回合的念头注入
    pub dream_store: Option<Arc<RwLock<DreamState>>>,
    /// 重连请求发送通道（用于热切换触发重连）
    /// 使用 broadcast 支持多消费者，Handler 和 Agent 都通过它通信
    pub reconnect_tx: Option<broadcast::Sender<ReconnectRequest>>,
    /// 死亡事件广播通道（用于 SSE 实时推送）
    pub death_event_tx: broadcast::Sender<ServerMessage>,
    /// Tick 更新广播通道（用于 SSE 实时推送，仅发送 tick_id）
    pub tick_update_tx: broadcast::Sender<i64>,
    pub runtime_mode: crate::config::RuntimeMode,
    pub narrative_config: std::sync::Arc<RwLock<Option<cyber_jianghu_protocol::NarrativeConfig>>>,
    pub is_dead: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 自动重生延迟 ticks（从 AgentDied 消息读取，0 = 不自动重生）
    pub rebirth_delay_ticks: std::sync::Arc<std::sync::atomic::AtomicI32>,
    /// 重生完成通知：auto-rebirth 成功后 notify，唤醒 tick 循环 select!
    pub rebirth_notify: std::sync::Arc<tokio::sync::Notify>,
    /// auto-rebirth 产出的 new_agent_id（task 写入，main loop 读取）
    pub pending_rebirth_agent_id: Arc<RwLock<Option<uuid::Uuid>>>,
    /// auto-rebirth 产出的服务端权威 system_prompt
    pub pending_rebirth_system_prompt: Arc<RwLock<Option<String>>>,
    /// 自动重生开关（运行时可热切换）
    pub auto_rebirth: std::sync::Arc<std::sync::atomic::AtomicBool>,

    /// 自动注册倒计时截止时刻（等待注册态布防；Some 时 setup/status 暴露剩余秒数，
    /// 面板显示倒计时；超时由 Agent 自动生成并注册角色）
    pub auto_register_deadline: Arc<RwLock<Option<std::time::Instant>>>,
    /// HTTP API 服务器实际端口（用于 Web 面板链接）
    pub actual_port: u16,
    /// LLM Client 容器（支持热重载时重建）
    pub llm_container:
        std::sync::Arc<tokio::sync::RwLock<Option<crate::runtime::claw::LlmClientContainer>>>,
    /// 上一次决策上下文快照（供 /api/v1/context enrichment 使用）
    pub decision_context_snapshot:
        std::sync::Arc<tokio::sync::RwLock<Option<DecisionContextSnapshot>>>,
    /// WorldStateStore（Agent 侧 WorldState 本地落存，供 Delta Engine 使用）
    pub world_state_store:
        Arc<std::sync::RwLock<Option<Arc<crate::component::state_store::WorldStateStore>>>>,
    /// 自更新器（GitHub Release；见 infra/updater.rs）
    pub updater: std::sync::Arc<crate::infra::updater::Updater>,
}

/// 决策上下文快照（lifecycle 每轮写入，HTTP API 读取）
#[derive(Debug, Clone)]
pub struct DecisionContextSnapshot {
    pub tick_id: i64,
    /// 完整 memory_context（三层记忆 + 生存/理智/延迟对话/托梦）
    pub memory_context: String,
    /// 行动历史滑窗
    pub summary_context: String,
    /// 行动结果学习
    pub outcome_section: String,
    /// 动作描述列表
    pub action_descriptions: String,
    /// 动作字段 schema
    pub action_field_hints: String,
    /// 上次执行结果
    pub last_execution_result: Option<ExecutionSummary>,
}

/// 执行结果摘要
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecutionSummary {
    pub action_type: String,
    pub success: bool,
    pub narrative: String,
}

/// HTTP 决策状态
pub struct HttpDecisionState {
    pub api_state: HttpApiState,
    pub intent_rx: Arc<Mutex<mpsc::Receiver<Intent>>>,
    pub ws_shared_state: Option<Arc<crate::runtime::claw::WsSharedState>>,
}

/// Intent 提交请求（数据驱动）
///
/// 客户端直接提供 action_data JSON，服务端直接透传。
/// 添加新的 action type 不需要修改服务端代码。
#[derive(Deserialize)]
pub struct IntentRequest {
    /// Intent 唯一 ID（可选，如果未提供则自动生成）
    pub intent_id: Option<String>,
    /// 动作类型（如 "休整", "说话", "移动" 等）
    pub action_type: String,
    /// Agent ID（可选，默认使用服务端配置的 agent_id）
    pub agent_id: Option<String>,
    /// Tick ID（可选，默认使用当前 tick）
    pub tick_id: Option<i64>,
    /// 思考日志（可选，Agent 的内心独白）
    pub thought_log: Option<String>,
    /// 动作数据（JSON，由客户端根据 action_type 构建完整数据）
    /// 服务端直接透传，不做任何解析或构建
    #[serde(default)]
    pub action_data: Option<serde_json::Value>,
}

// ============================================================================
// 决策函数
// ============================================================================

/// 创建 HTTP 决策函数
/// HTTP 决策函数
///
/// 工作流程：
/// 1. 收到 WorldState 后更新到 shared_state.current_state
/// 2. 已禁用 HTTP intent 入口，强制使用 WebSocket
/// 3. 超时返回 idle 意图，不阻塞游戏循环
pub fn http_decision(
    agent_id: Arc<RwLock<Uuid>>,
    state: Arc<HttpDecisionState>,
    _timeout_secs: u64, // 废弃固定值，改用动态计算
) -> impl Fn(&WorldState) -> BoxFuture<'static, Intent> + Send + Sync + 'static {
    move |world_state: &WorldState| {
        let world_state = world_state.clone();
        let state = state.clone();
        let agent_id_clone = agent_id.clone();

        Box::pin(async move {
            // 更新共享状态（供 HTTP API 读取）
            {
                let mut current = state.api_state.current_state.write().await;
                *current = Some(world_state.clone());

                let mut last_update = state.api_state.last_state_update.write().await;
                *last_update = Some(std::time::Instant::now());
            }

            // 更新 WorldStateStore（供 Delta Engine 使用）
            {
                let wss = state
                    .api_state
                    .world_state_store
                    .read()
                    .expect("rwlock poisoned")
                    .clone();
                if let Some(wss) = wss {
                    wss.update(world_state.clone()).await;
                }
            }

            // 触发叙事更新（异步，不阻塞）
            state.api_state.maybe_update_narratives(&world_state).await;

            // 广播 Tick 更新事件（供 Web Panel SSE 实时刷新）
            if let Err(e) = state.api_state.tick_update_tx.send(world_state.tick_id) {
                tracing::warn!("tick_update_tx.send 失败（receiver 可能已 drop）：{e:?}");
            }

            if let Some(ref ws_state) = state.ws_shared_state {
                ws_state.broadcast_tick(&world_state);
            }

            // 等待外部决策
            let mut rx = state.intent_rx.lock().await;

            // 计算动态超时时间：tick_duration_secs * 0.8
            // 从 GameRules 获取真实的 tick_duration_secs（默认 60 秒）
            // 给 OpenClaw 足够的时间进行决策
            let tick_duration = state
                .api_state
                .tick_duration_secs
                .load(std::sync::atomic::Ordering::Relaxed);
            let dynamic_timeout = (tick_duration as f64 * 0.8) as u64;

            tracing::info!(
                "[http] Waiting for intent, tick={}, timeout={}s",
                world_state.tick_id,
                dynamic_timeout
            );

            // 消费队列中过期的意图
            loop {
                match rx.try_recv() {
                    Ok(intent) if intent.tick_id < world_state.tick_id => {
                        tracing::warn!("[http] Dropped expired intent for tick {}", intent.tick_id);
                        continue;
                    }
                    Ok(intent) => {
                        // 发现当前或未来 tick 的意图，直接返回
                        tracing::info!(
                            "[http] Found queued intent for tick {}, action={}",
                            intent.tick_id,
                            intent.action_type
                        );
                        return intent;
                    }
                    Err(_) => break,
                }
            }

            match tokio::time::timeout(Duration::from_secs(dynamic_timeout), rx.recv()).await {
                Ok(Some(intent)) => {
                    tracing::info!(
                        "[http] Received intent for tick {}, action={}",
                        intent.tick_id,
                        intent.action_type
                    );
                    intent
                }
                Ok(None) => {
                    error!("[http] Channel closed, defaulting to idle");
                    let guard = agent_id_clone.read().await;
                    let id = *guard;
                    Intent::new(id, world_state.tick_id, "休整", None)
                }
                Err(_) => {
                    // 超时是正常的（表示没有外部决策）
                    let guard = agent_id_clone.read().await;
                    let id = *guard;
                    Intent::new(id, world_state.tick_id, "休整", None)
                }
            }
        })
    }
}

// ============================================================================
// HTTP Server
// ============================================================================
