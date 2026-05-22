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
// - GET/POST /api/v1/config/llm-disabled  - LLM 停止状态
// - GET/POST /api/v1/config/auto-rebirth  - 自动重生开关
//
// 架构设计：
// - 数据驱动 COI 原则：AI 组件都是可选注入，按需初始化
// - HTTP API 是辅助功能，WebSocket 是 OpenClaw 与 Agent 的主通道
// - 并发安全：所有可变状态都使用 tokio 的读写锁保护

pub mod cognitive_context;
mod context;
mod dto;
pub(crate) mod handlers;
pub mod intent_history;
pub mod service;
pub mod soul_cycle_recorder;
pub mod thinking_log;

use axum::{
    Router,
    routing::{get, post},
};
use cyber_jianghu_protocol::{Intent, ServerMessage, WorldState};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use tracing::{error, info};
use uuid::Uuid;

use anyhow::Context;

/// 重连请求（通过 channel 发送给主循环）
#[derive(Debug, Clone)]
pub struct ReconnectRequest {
    pub ws_url: String,
    pub agent_id: Option<Uuid>,
}

// 导入 handlers 中的 DreamState
pub(crate) use handlers::DreamState;

// 导入 AI 模块类型
use crate::component::memory::{MemoryManager, MemoryManagerConfig};
use crate::component::persona::dynamic_persona::ThreadSafePersona;
use crate::component::social::{DialogueClient, DialogueEventHandler};
use crate::component::social::{NarrativeGenerator, RelationshipStore};
use crate::soul::reflector::Validator;

// 重导出 review 模块的公共 API（已迁移至 soul::reflector::store）
pub use crate::soul::reflector::store::ReviewStore;
pub type ReviewState = std::sync::Arc<ReviewStore>;

// 重导出 context 模块的公共 API
pub use context::{
    AttributesGlimpse, ContextResponse, create_attributes_glimpse,
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
    /// 动态人设（可选）
    pub dynamic_persona: Option<Arc<ThreadSafePersona>>,
    /// 审查存储，管理待审查意图和审查结果（仅 Player Agent 使用）
    pub review_store: Option<Arc<ReviewStore>>,
    /// Intent 历史存储，记录每个 tick 的 thought_log 和 observer_thought
    pub intent_history: Arc<RwLock<Option<Arc<intent_history::IntentHistoryStore>>>>,
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
    /// 自动重生开关（运行时可热切换）
    pub auto_rebirth: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
    /// 动作类型（如 "休息", "说话", "移动" 等）
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
                let wss = state.api_state.world_state_store.read().unwrap().clone();
                if let Some(wss) = wss {
                    wss.update(world_state.clone()).await;
                }
            }

            // 触发叙事更新（异步，不阻塞）
            state.api_state.maybe_update_narratives(&world_state).await;

            // 持久化 events_log 到 IntentHistory（供经历日志查询）
            if let Some(history) = state.api_state.intent_history.read().await.as_ref() {
                let world_time_str = serde_json::to_string(&world_state.world_time).ok();
                for (i, event) in world_state.events_log.iter().enumerate() {
                    let tick_id =
                        world_state.tick_id - (world_state.events_log.len() - i - 1) as i64;
                    history
                        .record_event(tick_id, &event.description, world_time_str.clone())
                        .await;
                }
            }

            // 广播 Tick 更新事件（供 Web Panel SSE 实时刷新）
            let _ = state.api_state.tick_update_tx.send(world_state.tick_id);

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
                    Intent::new(id, world_state.tick_id, "休息", None)
                }
                Err(_) => {
                    // 超时是正常的（表示没有外部决策）
                    let guard = agent_id_clone.read().await;
                    let id = *guard;
                    Intent::new(id, world_state.tick_id, "休息", None)
                }
            }
        })
    }
}

// ============================================================================
// HTTP Server
// ============================================================================

/// 创建 HTTP API Router
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
        // === 认知上下文端点（引导 OpenClaw 按阶段推理）===
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
            "/api/v1/memory/daily-summaries",
            get(handlers::get_daily_summaries_handler),
        ) // 获取每日摘要
        .route(
            "/api/v1/memory/search",
            post(handlers::search_memory_handler),
        ) // 搜索记忆（语义搜索已实现，见 MemoryManager::recall_archived）
        .route("/api/v1/memory", post(handlers::store_memory_handler)) // 存储记忆
        // === 意图验证端点 ===
        .route("/api/v1/validate", post(handlers::validate_intent_handler)) // 验证意图是否符合人设
        // === 角色注册端点 ===
        .route(
            "/api/v1/character/generate",
            post(handlers::generate_character_handler),
        ) // LLM 一键生成角色
        .route(
            "/api/v1/character/register",
            post(handlers::register_character_handler),
        ) // 创建新角色（转发到 Server）
        // === 角色信息端点 ===
        .route(
            "/api/v1/attribute-meta",
            get(handlers::get_attribute_meta_handler),
        ) // 属性元数据（分类）
        .route("/api/v1/character", get(handlers::get_character_handler)) // 获取角色信息
        .route(
            "/api/v1/character/soul-cycles",
            get(handlers::get_soul_cycles_handler),
        ) // 获取三魂循环完整记录（本地内存）
        .route(
            "/api/v1/character/biography",
            get(handlers::get_biography_handler),
        ) // 获取角色传记（缓存）
        .route(
            "/api/v1/character/biography",
            post(handlers::generate_biography_handler),
        ) // 生成角色传记（LLM 纪传体）
        .route(
            "/api/v1/character/rebirth",
            post(handlers::rebirth_character_handler),
        ) // 转生（强制归隐重新注册）
        .route("/api/v1/character/dream", get(handlers::get_dream_handler)) // 获取托梦状态
        .route(
            "/api/v1/character/dream",
            post(handlers::dream_character_handler),
        ) // 托梦（持续 n 回合的念头注入）
        .route(
            "/api/v1/character/dream/records",
            get(handlers::get_dream_records_handler),
        )
        // === 多角色管理端点 ===
        .route("/api/v1/characters", get(handlers::list_characters_handler)) // 获取所有角色列表
        .route(
            "/api/v1/characters/switch",
            post(handlers::switch_character_handler),
        ) // 切换当前角色
        .route(
            "/api/v1/characters/{agent_id}",
            get(handlers::get_character_by_id_handler),
        ) // 获取指定角色详情
        // === 审查系统端点（Player Agent 提供，Observer Agent 调用）===
        .route(
            "/api/v1/review/pending",
            get(crate::soul::reflector::store::get_pending_reviews),
        ) // 获取待审查意图
        .route(
            "/api/v1/review/{intent_id}",
            post(crate::soul::reflector::store::submit_review),
        ) // 提交审查结果
        .route(
            "/api/v1/review/{intent_id}/status",
            get(crate::soul::reflector::store::get_review_status),
        ) // 获取审查状态
        // === 实时事件端点（SSE）===
        .route("/api/v1/events", get(handlers::death_events_handler)) // 死亡事件 SSE 流
        // === 配置管理端点 ===
        .route("/api/v1/config", get(handlers::get_config_handler)) // 获取当前配置
        .route(
            "/api/v1/config/llm-disabled",
            get(handlers::get_llm_disabled_handler),
        ) // 获取 LLM 停止状态
        .route(
            "/api/v1/config/llm-disabled",
            post(handlers::set_llm_disabled_handler),
        ) // 设置 LLM 停止状态
        .route(
            "/api/v1/config/auto-rebirth",
            get(handlers::get_auto_rebirth_handler),
        ) // 获取自动重生开关
        .route(
            "/api/v1/config/auto-rebirth",
            post(handlers::set_auto_rebirth_handler),
        ) // 设置自动重生开关
        .route("/api/v1/actions", get(handlers::get_actions_handler)) // 获取动作类型映射
        .route("/api/v1/metrics", get(handlers::get_metrics_handler)) // LLM 性能指标
        .route(
            "/api/v1/config/reload",
            post(handlers::reload_config_handler),
        ) // 热重载配置
        .route("/api/v1/config/server", post(handlers::set_server_handler)) // 设置服务器地址
        // === 引导状态端点 ===
        .route("/api/v1/setup/status", get(handlers::setup_status_handler)) // 获取引导状态
        // === LLM 配置端点 ===
        .route(
            "/api/v1/config/llm/providers",
            get(handlers::get_llm_providers_handler),
        ) // 获取支持的 LLM Provider 列表
        .route(
            "/api/v1/config/llm/providers/openclaw/defaults",
            get(handlers::get_openclaw_defaults_handler),
        ) // 获取 OpenClaw 默认配置（仅当选择 openclaw 时调用）
        .route("/api/v1/config/llm", get(handlers::get_llm_config_handler)) // 获取当前 LLM 配置
        .route(
            "/api/v1/config/llm",
            post(handlers::update_llm_config_handler),
        ) // 更新 LLM 配置
        .route(
            "/api/v1/config/llm/usage",
            get(handlers::get_llm_usage_handler),
        ) // 获取 LLM Token 累计使用统计
}

/// 获取静态文件服务目录
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
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            return Err(anyhow::anyhow!(
                "无法绑定端口 {} ({}): 请检查是否有旧进程仍在运行，或使用其他端口",
                port,
                e
            ));
        }
    };
    let local_addr = listener.local_addr()?;
    info!("[http] API Server listening on {}", local_addr);
    info!("[http] HTTP_PORT={}", local_addr.port());
    info!("[http] Web Panel: http://127.0.0.1:{}/", local_addr.port());
    info!(
        "[http] - Create character: http://127.0.0.1:{}/create.html",
        local_addr.port()
    );
    info!(
        "[http] - Character info:  http://127.0.0.1:{}/character.html",
        local_addr.port()
    );
    info!(
        "[http] - Settings:        http://127.0.0.1:{}/settings.html",
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
/// 用于默认初始化，所有事件处理器都是空操作
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
        if let Some(home) = dirs::home_dir() {
            let narrative_path = home
                .join(".cyber-jianghu")
                .join("config")
                .join("narrative_config.json");
            if narrative_path.exists() {
                std::fs::read_to_string(&narrative_path)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
            } else {
                None
            }
        } else {
            None
        }
    };

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

    let auto_rebirth_init = crate::config::Config::from_file(&config_path)
        .map(|c| c.runtime.auto_rebirth)
        .unwrap_or(true);

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
        dynamic_persona: None,
        review_store: None, // 由 Player Agent 通过 builder 设置
        intent_history: Arc::new(RwLock::new(match intent_history::IntentHistoryStore::open(
            current_agent_id,
            &data_dir_clone.join(format!("intent_history_{}.db", current_agent_id)),
        ) {
            Ok(store) => Some(Arc::new(store)),
            Err(e) => {
                tracing::error!("Failed to open IntentHistoryStore: {}", e);
                // Depending on context, we might want to panic here if it's a hard requirement,
                // but since this is state creation, we'll log it and leave it None to avoid crashing
                // the whole node on startup if a single agent's DB is corrupted.
                None
            }
        })),
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
        auto_rebirth: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(auto_rebirth_init)),
        actual_port,
        llm_container: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        decision_context_snapshot: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        world_state_store: Arc::new(std::sync::RwLock::new(None)),
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
    pub fn with_dynamic_persona(mut self, persona: Arc<ThreadSafePersona>) -> Self {
        self.dynamic_persona = Some(persona);
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

    /// 在 Tick 处理后异步更新关系描述
    pub async fn maybe_update_narratives(&self, world_state: &WorldState) {
        let Some(generator) = &self.narrative_generator else {
            return; // 没有 LlmClient，跳过
        };

        let store_guard = self.relationship_store.read().unwrap();
        let Some(store) = store_guard.as_ref() else {
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

    /// 刷新设备认证令牌（HTTP 401 时调用）
    ///
    /// 调用 `POST {server_http_url}/api/v1/agent/connect` 获取新的 auth_token，
    /// 然后更新本地 device_config 并持久化到 device.yaml。
    pub async fn refresh_auth_token(&self) -> anyhow::Result<()> {
        // 1. 获取当前设备配置
        let device = self.device_config.read().await;
        let device = device.as_ref().context("设备身份未初始化")?;
        let device_id = device.device_id;
        let _ = device; // 释放锁，避免死锁

        // 2. 获取 HTTP URL
        let http_url = self.server_http_url.read().await.clone();
        let url = format!("{}/api/v1/agent/connect", http_url);

        // 3. 调用 connect API 获取新 token
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .json(&serde_json::json!({ "device_id": device_id }))
            .send()
            .await
            .context("刷新令牌请求失败")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("刷新令牌失败 {}: {}", status, body);
        }

        #[derive(Deserialize)]
        struct ConnectResponse {
            auth_token: String,
        }

        let result: ConnectResponse = response.json().await.context("解析刷新令牌响应失败")?;

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
