// ============================================================================
// HTTP Decision - HTTP API 服务器（辅助功能）
// ============================================================================
//
// HTTP API 用于数据查询、Web 面板、调试等辅助功能。
// OpenClaw（外置大脑）必须通过 WebSocket 连接 Agent，确保 Tick 实时同步。
//
// 可用端点：
// - GET  /api/v1               - API 列表和使用规范（发现端点）
// - GET  /api/v1/health        - 健康检查
// - GET  /api/v1/state         - 获取当前 WorldState
// - GET  /api/v1/context       - 获取格式化的上下文（Markdown）
// - GET  /api/v1/attributes    - 梦中一瞥：获取属性数值（禁止存储到记忆）
// - POST /api/v1/intent        - 提交 Intent (已禁用，强制使用 WebSocket)
// - GET  /api/v1/relationship/list  - 获取所有关系
// - GET  /api/v1/relationship/{id}   - 获取特定关系
// - POST /api/v1/relationship       - 更新关系
// - GET  /api/v1/lifespan      - 获取寿命状态
// - GET  /api/v1/memory/recent - 获取近期记忆
// - POST /api/v1/memory/search - 搜索记忆（语义搜索待实现）
// - POST /api/v1/memory        - 存储记忆
// - POST /api/v1/validate      - 验证 Intent
// - GET  /api/v1/review/pending    - 获取待审查意图列表（Player Agent 提供）
// - POST /api/v1/review/{intent_id} - 提交审查结果（Observer Agent 调用）
// - GET  /api/v1/review/{intent_id}/status - 获取审查状态
//
// 架构设计：
// - 数据驱动 COI 原则：AI 组件都是可选注入，按需初始化
// - HTTP API 是辅助功能，WebSocket 是 OpenClaw 与 Agent 的主通道
// - 并发安全：所有可变状态都使用 tokio 的读写锁保护

pub mod cognitive_context;
mod context;
mod dto;
mod handlers;
pub mod intent_history;
pub mod review;
pub mod service;

use axum::{
    Router,
    routing::{get, post},
};
use cyber_jianghu_protocol::{Intent, WorldState};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{error, info};
use uuid::Uuid;

/// 重连请求（通过 channel 发送给主循环）
#[derive(Debug, Clone)]
pub struct ReconnectRequest {
    pub ws_url: String,
}

// 导入 handlers 中的 DreamState
pub use handlers::DreamState;

// 导入 AI 模块类型
use crate::ai::cognitive::narrative::NarrativeEngine;
use crate::ai::dialogue::{DialogueClient, DialogueEventHandler};
use crate::ai::lifespan::LifespanCalculator;
use crate::ai::memory::{MemoryManager, MemoryManagerConfig};
use crate::ai::persona::dynamic_persona::ThreadSafePersona;
use crate::ai::relationship::{NarrativeGenerator, RelationshipStore};
use crate::ai::validator::{RuleEngineValidator, Validator};

// 重导出 review 模块的公共 API
pub use review::{PendingReviewEntry, ReviewState, ReviewStore};

// 重导出 context 模块的公共 API
pub use context::{
    AttributesGlimpse, ContextResponse, create_attributes_glimpse, create_narrative_engine,
    generate_context_markdown_no_relationship,
};

// 重导出 cognitive_context 模块的公共 API
pub use cognitive_context::{
    AvailableActionInfo, CognitiveContext, CognitiveContextBuilder, CognitiveContextConfig,
    DecisionContext, Drive, MotivationContext, PerceptionContext, PlanningContext,
};

// ============================================================================
// 核心类型定义
// ============================================================================

/// HTTP API 配置
///
/// 配置 HTTP API 服务器的运行参数（辅助功能）
pub struct HttpDecisionConfig {
    /// 监听端口
    pub port: u16,
    /// 决策超时（秒），超过此时间返回 idle 意图
    pub timeout_secs: u64,
}

impl Default for HttpDecisionConfig {
    fn default() -> Self {
        Self {
            port: 0, // 0 = 随机端口
            timeout_secs: 55,
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
    /// 设备身份（device_id + auth_token）
    pub identity: Option<crate::config::IdentityConfig>,
    /// 配置文件路径（用于读取角色配置）
    pub config_path: PathBuf,

    // AI 组件（全部可选，支持按需注入）
    /// 对话客户端，处理 Agent 间对话
    pub dialogue_client: Option<Arc<DialogueClient>>,
    /// 关系存储，持久化存储与其他 Agent 的关系记忆
    pub relationship_store: Option<Arc<RelationshipStore>>,
    /// 寿命计算器，计算年龄和老化效果
    /// 需要 Mutex 支持内部状态修改
    pub lifespan_calculator: Option<Arc<Mutex<LifespanCalculator>>>,
    /// 记忆管理器，管理工作记忆、情景记忆和语义记忆
    /// 需要 Mutex 支持异步操作
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
    /// 意图验证器，验证意图是否符合人设
    pub intent_validator: Option<Arc<dyn Validator>>,
    /// 叙事生成器（可选，仅在有 LlmClient 时可用）
    pub narrative_generator: Option<Arc<NarrativeGenerator>>,
    /// 动态人设（可选）
    pub dynamic_persona: Option<Arc<ThreadSafePersona>>,
    /// 叙事引擎，将数值属性转换为叙事化描述
    pub narrative_engine: Option<Arc<NarrativeEngine>>,
    /// 审查存储，管理待审查意图和审查结果（仅 Player Agent 使用）
    pub review_store: Option<Arc<ReviewStore>>,
    /// Intent 历史存储，记录每个 tick 的 thought_log 和 observer_thought
    pub intent_history: Option<Arc<intent_history::IntentHistoryStore>>,
    /// 托梦存储，管理持续 n 回合的念头注入
    pub dream_store: Option<Arc<RwLock<DreamState>>>,
    /// 重连请求发送通道（用于热切换触发重连）
    pub reconnect_tx: Option<mpsc::Sender<ReconnectRequest>>,
}

/// HTTP 决策状态
pub struct HttpDecisionState {
    pub api_state: HttpApiState,
    pub intent_rx: Arc<Mutex<mpsc::Receiver<Intent>>>,
    pub ws_shared_state: Option<Arc<super::ws::WsSharedState>>,
}

/// Intent 提交请求（数据驱动）
///
/// 客户端直接提供 action_data JSON，服务端直接透传。
/// 添加新的 action type 不需要修改服务端代码。
#[derive(Deserialize)]
pub struct IntentRequest {
    /// Intent 唯一 ID（可选，如果未提供则自动生成）
    pub intent_id: Option<String>,
    /// 动作类型（如 "idle", "speak", "move" 等）
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

            // 触发叙事更新（异步，不阻塞）
            state.api_state.maybe_update_narratives(&world_state).await;

            if let Some(ref ws_state) = state.ws_shared_state {
                let tick_duration = state
                    .api_state
                    .tick_duration_secs
                    .load(std::sync::atomic::Ordering::Relaxed);
                let deadline =
                    Instant::now() + Duration::from_secs((tick_duration as f64 * 0.9) as u64);
                ws_state.broadcast_tick(&world_state, deadline);
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
                    Intent::idle(id, world_state.tick_id)
                }
                Err(_) => {
                    // 超时是正常的（表示没有外部决策）
                    let guard = agent_id_clone.read().await;
                    let id = *guard;
                    Intent::idle(id, world_state.tick_id)
                }
            }
        })
    }
}

// ============================================================================
// HTTP Server
// ============================================================================

/// 创建 HTTP API Router（供 claw 模式复用）
///
/// 返回包含所有数据访问 API 的 Router，需要调用者提供 HttpApiState
pub fn create_api_router() -> Router<HttpApiState> {
    Router::new()
        // === API 发现端点 ===
        .route("/api/v1", get(handlers::api_list_handler)) // API 列表和使用规范
        // === 基础端点 ===
        .route("/api/v1/health", get(handlers::health_handler)) // 健康检查
        .route("/api/v1/state", get(handlers::get_state_handler)) // 获取当前世界状态
        .route("/api/v1/context", get(handlers::get_context_handler)) // 获取格式化上下文
        .route("/api/v1/attributes", get(handlers::get_attributes_handler)) // 梦中一瞥：属性数值
        .route("/api/v1/tick", get(handlers::get_tick_status_handler)) // 获取 Tick 状态（轮询用）
        // === 认知上下文端点（引导 OpenClaw 四阶段推理）===
        .route(
            "/api/v1/cognitive",
            get(handlers::get_cognitive_context_handler),
        ) // 结构化认知上下文
        // === 关系管理端点 ===
        .route(
            "/api/v1/relationship/list",
            get(handlers::get_relationships_handler),
        ) // 获取所有关系
        .route(
            "/api/v1/relationship/{id}",
            get(handlers::get_relationship_handler),
        ) // 获取特定关系
        .route(
            "/api/v1/relationship",
            post(handlers::update_relationship_handler),
        ) // 更新关系
        // === 寿命端点 ===
        .route("/api/v1/lifespan", get(handlers::get_lifespan_handler)) // 获取寿命状态
        // === 记忆管理端点 ===
        .route(
            "/api/v1/memory/recent",
            get(handlers::get_recent_memory_handler),
        ) // 获取近期记忆
        .route(
            "/api/v1/memory/search",
            post(handlers::search_memory_handler),
        ) // 搜索记忆 TODO: 语义搜索待实现
        .route("/api/v1/memory", post(handlers::store_memory_handler)) // 存储记忆
        // === 意图验证端点 ===
        .route("/api/v1/validate", post(handlers::validate_intent_handler)) // 验证意图是否符合人设
        // === 角色注册端点 ===
        .route(
            "/api/v1/character/register",
            post(handlers::register_character_handler),
        ) // 创建新角色（转发到 Server）
        // === 角色信息端点 ===
        .route("/api/v1/character", get(handlers::get_character_handler)) // 获取角色信息
        .route(
            "/api/v1/character/experiences",
            get(handlers::get_experiences_handler),
        ) // 获取经历日志（分页）
        .route(
            "/api/v1/character/rebirth",
            post(handlers::rebirth_character_handler),
        ) // 转生（强制归隐重新注册）
        .route("/api/v1/character/dream", get(handlers::get_dream_handler)) // 获取托梦状态
        .route(
            "/api/v1/character/dream",
            post(handlers::dream_character_handler),
        ) // 托梦（持续 n 回合的念头注入）
        // === 多角色管理端点 ===
        .route("/api/v1/characters", get(handlers::list_characters_handler)) // 获取所有角色列表
        .route(
            "/api/v1/characters/switch",
            post(handlers::switch_character_handler),
        ) // 切换当前角色
        // === 审查系统端点（Player Agent 提供，Observer Agent 调用）===
        .route("/api/v1/review/pending", get(review::get_pending_reviews)) // 获取待审查意图
        .route("/api/v1/review/{intent_id}", post(review::submit_review)) // 提交审查结果
        .route(
            "/api/v1/review/{intent_id}/status",
            get(review::get_review_status),
        ) // 获取审查状态
        // === 配置管理端点 ===
        .route("/api/v1/config", get(handlers::get_config_handler)) // 获取当前配置
        .route(
            "/api/v1/config/reload",
            post(handlers::reload_config_handler),
        ) // 热重载配置
        .route("/api/v1/config/server", post(handlers::set_server_handler)) // 设置服务器地址
}

/// 获取静态文件服务目录（供 claw 模式复用）
pub fn get_static_serve_dir() -> PathBuf {
    let panel_path = PathBuf::from("crates/agent/static/panel");
    let panel_path_alt = PathBuf::from("static/panel");
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_default();
    let exe_panel_path = exe_dir.join("static/panel");

    if panel_path.exists() {
        panel_path
    } else if panel_path_alt.exists() {
        panel_path_alt
    } else {
        exe_panel_path
    }
}

/// 启动 HTTP API 服务器
///
/// 启动后监听指定端口，提供 RESTful API 供外部系统调用
/// 所有端点都需要从共享状态中获取对应的 AI 组件，如果组件未初始化
/// 则返回 503 SERVICE_UNAVAILABLE 错误
pub async fn run_http_server(port: u16, api_state: HttpApiState) -> anyhow::Result<()> {
    let app = create_api_router().with_state(api_state.clone());

    // 添加静态文件服务（用于 Web 面板）
    let serve_dir = get_static_serve_dir();
    let app = app.fallback_service(tower_http::services::ServeDir::new(serve_dir));

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local_addr = listener.local_addr()?;
    info!("[http] API Server listening on {}", local_addr);
    info!("[http] HTTP_PORT={}", local_addr.port());
    info!("[http] Web Panel: http://127.0.0.1:{}/", local_addr.port());
    info!(
        "[http] - Create character: http://127.0.0.1:{}/index.html",
        local_addr.port()
    );
    info!(
        "[http] - Character info:  http://127.0.0.1:{}/character.html",
        local_addr.port()
    );
    info!(
        "[http] - Management:      http://127.0.0.1:{}/manage.html",
        local_addr.port()
    );

    axum::serve(listener, app).await?;
    Ok(())
}

// ============================================================================
// 默认对话处理器
// ============================================================================

/// 空操作对话处理器
///
/// 用于 Claw 模式下的默认初始化，所有事件处理器都是空操作
/// 实际处理由外部系统（OpenClaw）通过 WebSocket + HTTP API 完成
#[derive(Debug, Default)]
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
/// - `identity`: 设备身份（device_id + auth_token）
///
/// # 返回值
///
/// - `(Arc<HttpDecisionState>, HttpApiState)`: 决策状态和 API 状态
///
/// 初始化策略：
/// - 关系存储：使用默认数据库路径初始化，失败则为 None
/// - 记忆管理器：使用无 LLM 版本（基础功能可用，语义搜索待实现），失败则为 None
/// - 寿命计算器：使用默认配置强制初始化
/// - 对话客户端：使用空操作处理器强制初始化
/// - 意图验证器：使用默认规则引擎验证器强制初始化
/// - 语义搜索：TODO: 下一阶段实现，需要 LLM 提供嵌入
///
/// # 注意
/// agent_id 是共享的，WebSocket 注册后会更新为服务器分配的真正 ID
///
/// # Arguments
/// * `config_path` - 配置文件完整路径（由调用者传入，确保与主程序一致）
pub fn create_http_state(
    agent_id: Arc<RwLock<Uuid>>,
    server_http_url: String,
    server_ws_url: String,
    identity: Option<crate::config::IdentityConfig>,
    reconnect_tx: Option<mpsc::Sender<ReconnectRequest>>,
    config_path: PathBuf,
    ws_shared_state: Option<Arc<super::ws::WsSharedState>>,
) -> (Arc<HttpDecisionState>, HttpApiState) {
    let (intent_tx, intent_rx) = mpsc::channel(100);

    // 初始化数据目录
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let data_dir = home.join(".cyber-jianghu").join("data");

    // 读取 agent_id（使用 block_in_place 在同步上下文中读取异步锁）
    let current_agent_id = {
        let guard = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(agent_id.read())
        });
        *guard
    }; // guard 在这里释放

    // 初始化关系存储
    let relationship_store =
        RelationshipStore::open(current_agent_id, &data_dir.join("relationships.db"))
            .ok()
            .map(Arc::new);

    // 初始化记忆管理器（无 LLM 版本，基础功能可用）
    // TODO: 语义搜索功能下一阶段实现，需要 LLM Client 提供嵌入能力
    let memory_config = MemoryManagerConfig {
        agent_id: current_agent_id,
        db_dir: data_dir.clone(),
        ..Default::default()
    };
    let memory_manager = MemoryManager::new(memory_config)
        .ok()
        .map(|m| Arc::new(Mutex::new(m)));

    // 初始化寿命计算器（使用默认配置）
    let lifespan_calculator = Some(Arc::new(Mutex::new(
        LifespanCalculator::with_default_config(),
    )));

    // 初始化对话客户端（使用空操作处理器）
    // 实际对话事件处理由外部系统通过 API 完成
    let dialogue_handler = Arc::new(NoopDialogueHandler);
    let dialogue_client = Some(Arc::new(DialogueClient::new(
        current_agent_id,
        dialogue_handler,
    )));

    // 初始化意图验证器（使用默认规则引擎验证器）
    let intent_validator =
        Some(Arc::new(RuleEngineValidator::with_default_config()) as Arc<dyn Validator>);

    // 初始化叙事引擎（用于属性叙事化描述）
    let narrative_engine = Some(Arc::new(create_narrative_engine()));

    let api_state = HttpApiState {
        current_state: Arc::new(RwLock::new(None)),
        last_state_update: Arc::new(RwLock::new(None)),
        intent_tx: intent_tx.clone(),
        agent_id,
        tick_duration_secs: Arc::new(std::sync::atomic::AtomicU64::new(60)), // 默认 60 秒，注册后更新
        server_http_url: Arc::new(RwLock::new(server_http_url)),
        server_ws_url: Arc::new(RwLock::new(server_ws_url)),
        identity,
        config_path,
        dialogue_client,
        relationship_store,
        lifespan_calculator,
        memory_manager,
        intent_validator,
        narrative_generator: None,
        dynamic_persona: None,
        narrative_engine,
        review_store: None, // 由 Player Agent 通过 builder 设置
        intent_history: Some(Arc::new(intent_history::IntentHistoryStore::new(100))),
        dream_store: Some(Arc::new(RwLock::new(DreamState::default()))),
        reconnect_tx, // 重连请求发送通道
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
        self.relationship_store = Some(store);
        self
    }

    /// 设置寿命计算器
    pub fn with_lifespan_calculator(mut self, calculator: LifespanCalculator) -> Self {
        self.lifespan_calculator = Some(Arc::new(Mutex::new(calculator)));
        self
    }

    /// 设置记忆管理器
    pub fn with_memory_manager(mut self, manager: MemoryManager) -> Self {
        self.memory_manager = Some(Arc::new(Mutex::new(manager)));
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
    pub fn with_dynamic_persona(mut self, persona: Arc<ThreadSafePersona>) -> Self {
        self.dynamic_persona = Some(persona);
        self
    }

    /// 设置叙事引擎
    pub fn with_narrative_engine(mut self, engine: NarrativeEngine) -> Self {
        self.narrative_engine = Some(Arc::new(engine));
        self
    }

    /// 设置审查存储（仅 Player Agent 使用）
    pub fn with_review_store(mut self, store: Arc<ReviewStore>) -> Self {
        self.review_store = Some(store);
        self
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

    /// 获取当前托梦内容（如果有）
    /// 每次调用会减少剩余回合数
    pub async fn consume_dream(&self) -> Option<String> {
        let dream_store = self.dream_store.as_ref()?;
        let mut dream = dream_store.write().await;

        if dream.remaining_ticks > 0 {
            let thought = dream.thought.clone();
            dream.remaining_ticks = dream.remaining_ticks.saturating_sub(1);

            if dream.remaining_ticks == 0 {
                info!("[dream] 托梦效果已结束");
                dream.thought = None;
            }

            thought
        } else {
            dream.thought = None;
            None
        }
    }

    /// 在 Tick 处理后异步更新关系描述
    pub async fn maybe_update_narratives(&self, world_state: &WorldState) {
        let Some(generator) = &self.narrative_generator else {
            return; // 没有 LlmClient，跳过
        };

        let Some(store) = &self.relationship_store else {
            return;
        };

        let Some(persona) = &self.dynamic_persona else {
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
}
