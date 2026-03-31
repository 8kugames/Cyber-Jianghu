// ============================================================================
// WebSocket 决策状态管理
// ============================================================================
//
// 管理 Agent 与外部调度器之间的共享状态
// - WorldState 广播
// - Intent 接收
// - Tick 时序管理
// ============================================================================

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{debug, warn};

use crate::models::{Intent, WorldState};
use uuid::Uuid;

use super::protocol::{DownstreamMessage, WsIntent};

// ============================================================================
// 常量
// ============================================================================

/// 默认 Tick 持续时间（秒）
pub const DEFAULT_TICK_DURATION_SECS: u64 = 60;

/// Tick 超时缓冲比例（实际截止时间 = tick_duration * 0.9）
pub const TICK_TIMEOUT_RATIO: f64 = 0.9;

// ============================================================================
// 重导出验证模块
// ============================================================================

pub use super::validation::{ValidationTaskParams, WsValidationRequest, spawn_validation_task};

// ============================================================================
// WebSocket 决策状态
// ============================================================================

/// WebSocket 决策状态
///
/// 管理 Agent 与外部调度器之间的通信状态
pub struct WsDecisionState {
    /// WorldState 广播通道（容量 1，只保留最新）
    pub state_tx: broadcast::Sender<Arc<WorldState>>,

    /// tick_closed 广播通道（容量 16）
    pub tick_closed_tx: broadcast::Sender<DownstreamMessage>,

    /// Server 消息广播通道（容量 32，用于透传 Server 下行消息）
    pub server_msg_tx: broadcast::Sender<DownstreamMessage>,

    /// Intent 接收通道
    pub intent_rx: mpsc::Receiver<WsIntent>,

    /// Intent 发送通道（用于 server.rs）
    pub intent_tx: mpsc::Sender<WsIntent>,

    /// 验证请求接收通道（容量 1）
    pub validation_rx: mpsc::Receiver<WsValidationRequest>,

    /// 验证请求发送通道（用于 server.rs）
    pub validation_tx: mpsc::Sender<WsValidationRequest>,

    /// 当前 Tick ID
    pub current_tick: Arc<AtomicI64>,

    /// Tick 截止时间（Unix timestamp, 毫秒）
    pub deadline_ms: Arc<AtomicU64>,

    /// Tick 持续时间（毫秒）
    pub tick_duration_ms: Arc<AtomicU64>,

    /// Agent ID
    pub agent_id: Arc<AtomicI64>, // 存储 as i64
}

impl WsDecisionState {
    /// 创建新的 WebSocket 决策状态
    pub fn new() -> Self {
        let (state_tx, _) = broadcast::channel(1);
        let (tick_closed_tx, _) = broadcast::channel(16);
        let (server_msg_tx, _) = broadcast::channel(32);
        let (intent_tx, intent_rx) = mpsc::channel(16);
        let (validation_tx, validation_rx) = mpsc::channel(1); // 容量 1，强制背压

        Self {
            state_tx,
            tick_closed_tx,
            server_msg_tx,
            intent_rx,
            intent_tx,
            validation_rx,
            validation_tx,
            current_tick: Arc::new(AtomicI64::new(0)),
            deadline_ms: Arc::new(AtomicU64::new(0)),
            tick_duration_ms: Arc::new(AtomicU64::new(DEFAULT_TICK_DURATION_SECS * 1000)),
            agent_id: Arc::new(AtomicI64::new(0)),
        }
    }

    /// 获取当前 Tick ID
    pub fn get_current_tick(&self) -> i64 {
        self.current_tick.load(Ordering::Relaxed)
    }

    /// 设置当前 Tick ID
    pub fn set_current_tick(&self, tick_id: i64) {
        self.current_tick.store(tick_id, Ordering::Relaxed);
    }

    /// 获取 Tick 截止时间
    pub fn get_deadline(&self) -> Instant {
        let deadline_ms = self.deadline_ms.load(Ordering::Relaxed);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        if deadline_ms > now_ms {
            Instant::now() + Duration::from_millis(deadline_ms - now_ms)
        } else {
            Instant::now()
        }
    }

    /// 设置 Tick 截止时间
    pub fn set_deadline(&self, deadline: Instant) {
        let now = Instant::now();
        if deadline > now {
            let duration_ms = (deadline - now).as_millis() as u64;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            self.deadline_ms
                .store(now_ms + duration_ms, Ordering::Relaxed);
        }
    }

    /// 设置 Tick 持续时间
    pub fn set_tick_duration(&self, duration: Duration) {
        self.tick_duration_ms
            .store(duration.as_millis() as u64, Ordering::Relaxed);
    }

    /// 获取 Tick 持续时间
    pub fn get_tick_duration(&self) -> Duration {
        Duration::from_millis(self.tick_duration_ms.load(Ordering::Relaxed))
    }

    /// 设置 Agent ID
    pub fn set_agent_id(&self, agent_id: Uuid) {
        // 将 UUID 转换为 i64（取前 8 字节）
        let bytes = agent_id.as_bytes();
        let i = i64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        self.agent_id.store(i, Ordering::Relaxed);
    }

    /// 获取 Agent ID
    pub fn get_agent_id(&self) -> Uuid {
        let i = self.agent_id.load(Ordering::Relaxed);
        let bytes = i.to_be_bytes();
        Uuid::from_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], 0, 0,
            0, 0, 0, 0, 0, 0, // 剩余字节填 0
        ])
    }

    /// 广播 Tick 消息
    pub fn broadcast_tick(&self, world_state: &WorldState, deadline: Instant) {
        self.set_current_tick(world_state.tick_id);
        self.set_deadline(deadline);

        let state = Arc::new(world_state.clone());

        // 广播给所有订阅者
        match self.state_tx.send(state) {
            Ok(n) => debug!("Broadcast tick {} to {} clients", world_state.tick_id, n),
            Err(_) => debug!("No clients connected for tick {}", world_state.tick_id),
        }
    }

    /// 等待接收 Intent（带超时）
    pub async fn recv_intent(&mut self, deadline: Instant) -> Option<WsIntent> {
        let timeout_duration = deadline.saturating_duration_since(Instant::now());

        match tokio::time::timeout(timeout_duration, self.intent_rx.recv()).await {
            Ok(Some(intent)) => {
                // 校验 tick_id
                let current_tick = self.get_current_tick();
                if intent.tick_id < current_tick {
                    warn!(
                        "Dropped expired intent for tick {} (current: {})",
                        intent.tick_id, current_tick
                    );
                    return None;
                }
                Some(intent)
            }
            Ok(None) => {
                debug!("Intent channel closed");
                None
            }
            Err(_) => {
                // 超时
                None
            }
        }
    }

    /// 创建 tick_closed 消息
    pub fn create_tick_closed_message(&self, tick_id: i64, reason: &str) -> DownstreamMessage {
        let tick_duration_ms = self.tick_duration_ms.load(Ordering::Relaxed);

        DownstreamMessage::TickClosed {
            tick_id,
            reason: reason.to_string(),
            next_tick_in_ms: tick_duration_ms,
        }
    }

    /// 广播 tick_closed 消息给所有客户端
    pub fn broadcast_tick_closed(&self, tick_id: i64, reason: &str) {
        let msg = self.create_tick_closed_message(tick_id, reason);
        match self.tick_closed_tx.send(msg) {
            Ok(n) => debug!(
                "Broadcast tick_closed for tick {} to {} clients (reason: {})",
                tick_id, n, reason
            ),
            Err(_) => debug!("No clients connected for tick_closed"),
        }
    }

    /// 启动验证任务（后台运行）
    ///
    /// 验证任务从 validation_rx 接收验证请求，处理后：
    /// - 通过：转发到 intent_tx
    /// - 拒绝：通过 ws_tx 发送 ServerError
    pub fn spawn_validation_task(
        &mut self,
        shared_state: WsSharedState,
    ) -> tokio::task::JoinHandle<()> {
        let params = ValidationTaskParams {
            validation_rx: std::mem::replace(&mut self.validation_rx, mpsc::channel(1).1),
            intent_tx: shared_state.intent_tx.clone(),
            current_tick: self.current_tick.clone(),
            submitted_tick: shared_state.submitted_tick.clone(),
            intent_validator: shared_state.intent_validator.clone(),
            persona: shared_state.persona.clone(),
        };

        spawn_validation_task(params)
    }
}

impl Default for WsDecisionState {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 共享状态（用于 WebSocket Server）
// ============================================================================

/// WebSocket 共享状态（Clone 友好）
#[derive(Clone)]
pub struct WsSharedState {
    /// WorldState 广播通道
    pub state_tx: broadcast::Sender<Arc<WorldState>>,

    /// tick_closed 广播通道
    pub tick_closed_tx: broadcast::Sender<DownstreamMessage>,

    /// Server 消息广播通道（用于透传 Server 下行消息）
    pub server_msg_tx: broadcast::Sender<DownstreamMessage>,

    /// Intent 发送通道
    pub intent_tx: mpsc::Sender<WsIntent>,

    /// 上行消息发送通道（用于 OpenClawBridge 发送消息到 OpenClaw）
    /// Agent -> OpenClaw 方向
    pub upstream_tx: mpsc::Sender<super::protocol::UpstreamMessage>,

    /// LLM 响应通道（OpenClaw -> Agent）
    /// 当 OpenClaw 返回 LLM 响应时，通过此通道通知 OpenClawBridge
    pub llm_response_tx: mpsc::Sender<(String, Result<String, String>)>,

    /// LLM 响应接收通道（由 OpenClawBridge 使用）
    #[allow(clippy::type_complexity)]
    pub llm_response_rx:
        Arc<std::sync::Mutex<Option<mpsc::Receiver<(String, Result<String, String>)>>>>,

    /// 上行消息接收通道（从 WsDecisionState 转移所有权）
    /// 仅在第一个 WebSocket 连接时使用（单连接限制）
    pub upstream_rx:
        Arc<std::sync::Mutex<Option<mpsc::Receiver<super::protocol::UpstreamMessage>>>>,

    /// 当前 Tick ID
    pub current_tick: Arc<AtomicI64>,

    /// Tick 截止时间（Unix timestamp, 毫秒）
    pub deadline_ms: Arc<AtomicU64>,

    /// Tick 持续时间（毫秒）
    pub tick_duration_ms: Arc<AtomicU64>,

    /// Agent ID
    pub agent_id: Arc<AtomicI64>,

    /// 叙事引擎（可选，用于生成上下文）
    pub narrative_engine: Option<Arc<crate::soul::actor::narrative::NarrativeEngine>>,

    /// 认知上下文构建器（可选，用于生成四阶段认知上下文）
    pub cognitive_context_builder:
        Option<Arc<crate::infra::api::cognitive_context::CognitiveContextBuilder>>,

    /// OpenClaw 连接状态（单连接限制）
    pub openclaw_connected: Arc<AtomicBool>,

    /// 是否允许非 localhost 连接
    /// Docker 部署时需要设为 true，允许宿主机访问
    pub allow_external_connections: bool,

    // === 意图验证相关字段 ===
    /// 意图验证器
    ///
    /// 用于验证 OpenClaw 提交的意图是否符合人设和世界观规则。
    /// 使用 `RwLock<Option<...>>` 支持运行时动态更新验证器。
    ///
    /// # 验证流程
    /// 1. OpenClaw 通过 WebSocket 提交意图
    /// 2. 验证任务检查意图是否符合人设（persona）
    /// 3. 通过则转发到 intent_tx，否则返回 ServerError{ValidationFailed}
    ///
    /// # 降级策略
    /// - 无验证器（None）：直接转发
    /// - 验证超时（10秒）：允许通过
    /// - LLM 错误：允许通过
    ///
    /// # 更新方式
    /// 通过 `POST /api/v1/config` 或 Agent 初始化时设置
    pub intent_validator: Arc<RwLock<Option<Arc<dyn crate::soul::reflector::Validator>>>>,

    /// 人设信息
    ///
    /// 存储角色的性别、年龄、性格特点、三观倾向等信息，
    /// 用于意图验证器判断行为是否符合人设。
    /// 使用 `RwLock<Option<...>>` 支持运行时动态更新。
    ///
    /// # 字段说明
    /// - `gender`: 性别（如 "男"、"女"）
    /// - `age`: 年龄（0-255）
    /// - `personality`: 性格特点列表（如 ["沉稳", "重情义"]）
    /// - `values`: 三观倾向（如 ["江湖道义为先"]）
    ///
    /// # 更新时机
    /// - 角色注册时初始化
    /// - 角色性格演变时更新（通过事件反馈）
    pub persona: Arc<RwLock<Option<crate::soul::reflector::PersonaInfo>>>,

    /// 已提交的 tick_id（CAS 去重，-1 表示未提交）
    ///
    /// 使用 CAS (Compare-And-Swap) 操作原子性地防止同一 tick 重复提交。
    /// 新 tick 开始时重置为 -1。
    pub submitted_tick: Arc<AtomicI64>,

    /// 验证请求发送通道（容量 1，强制背压）
    pub validation_tx: mpsc::Sender<WsValidationRequest>,
}

impl WsSharedState {
    pub fn broadcast_tick(&self, world_state: &WorldState, deadline: Instant) {
        // 新 tick 开始，重置 submitted_tick 允许新提交
        self.submitted_tick.store(-1, Ordering::Release);

        self.current_tick
            .store(world_state.tick_id, Ordering::Relaxed);

        let now = Instant::now();
        if deadline > now {
            let duration_ms = (deadline - now).as_millis() as u64;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            self.deadline_ms
                .store(now_ms + duration_ms, Ordering::Relaxed);
        }

        let state = Arc::new(world_state.clone());

        match self.state_tx.send(state) {
            Ok(n) => debug!("Broadcast tick {} to {} clients", world_state.tick_id, n),
            Err(_) => debug!("No clients connected for tick {}", world_state.tick_id),
        }
    }
}

impl From<&WsDecisionState> for WsSharedState {
    fn from(state: &WsDecisionState) -> Self {
        // 从环境变量读取是否允许外部连接
        // Docker 部署时需要允许宿主机访问
        let allow_external_connections = std::env::var("CYBER_JIANGHU_WS_ALLOW_EXTERNAL")
            .map(|v| v == "1" || v == "true" || v == "yes")
            .unwrap_or(false);

        let (upstream_tx, upstream_rx) = mpsc::channel(16);
        let (llm_response_tx, llm_response_rx) = mpsc::channel(16);

        Self {
            state_tx: state.state_tx.clone(),
            tick_closed_tx: state.tick_closed_tx.clone(),
            server_msg_tx: state.server_msg_tx.clone(),
            intent_tx: state.intent_tx.clone(),
            current_tick: state.current_tick.clone(),
            deadline_ms: state.deadline_ms.clone(),
            tick_duration_ms: state.tick_duration_ms.clone(),
            agent_id: state.agent_id.clone(),
            narrative_engine: None,
            cognitive_context_builder: None,
            openclaw_connected: Arc::new(AtomicBool::new(false)),
            allow_external_connections,
            // 新增验证相关字段
            intent_validator: Arc::new(RwLock::new(None)),
            persona: Arc::new(RwLock::new(None)),
            submitted_tick: Arc::new(AtomicI64::new(-1)), // -1 表示未提交
            validation_tx: state.validation_tx.clone(),
            upstream_tx,
            llm_response_tx,
            llm_response_rx: Arc::new(std::sync::Mutex::new(Some(llm_response_rx))),
            upstream_rx: Arc::new(std::sync::Mutex::new(Some(upstream_rx))),
        }
    }
}

impl WsSharedState {
    /// 获取当前 Tick ID
    pub fn get_current_tick(&self) -> i64 {
        self.current_tick.load(Ordering::Relaxed)
    }

    /// 获取 Tick 截止时间（Unix timestamp, 毫秒）
    pub fn get_deadline_ms(&self) -> u64 {
        self.deadline_ms.load(Ordering::Relaxed)
    }

    /// 获取 Tick 持续时间（毫秒）
    pub fn get_tick_duration_ms(&self) -> u64 {
        self.tick_duration_ms.load(Ordering::Relaxed)
    }

    /// 生成叙事化上下文
    ///
    /// 如果配置了叙事引擎，使用叙事引擎生成；否则返回 None
    pub fn generate_context(&self, world_state: &WorldState) -> Option<String> {
        use crate::soul::actor::narrative::NarrativeEngine;

        // 获取叙事引擎（配置的或默认的）
        let engine: &NarrativeEngine = self.narrative_engine.as_deref().unwrap_or_else(|| {
            // 使用静态默认引擎
            static DEFAULT_ENGINE: std::sync::OnceLock<NarrativeEngine> =
                std::sync::OnceLock::new();
            DEFAULT_ENGINE.get_or_init(NarrativeEngine::with_builtin_config)
        });

        // 生成简化上下文（不包含关系信息）
        Some(
            crate::infra::api::generate_context_markdown_no_relationship(
                world_state,
                engine,
                None, // WebSocket 状态不包含托梦
            ),
        )
    }

    /// 生成四阶段认知上下文
    ///
    /// 如果配置了认知上下文构建器，使用构建器生成；否则返回 None
    ///
    /// 认知上下文用于引导 OpenClaw 进行四阶段推理：
    /// 1. Perception (感知): 理解当前世界状态
    /// 2. Motivation (动机): 基于人设生成内在驱动力
    /// 3. Planning (规划): 制定行动计划
    /// 4. Decision (决策): 选择最终行动
    pub fn generate_cognitive_context(
        &self,
        world_state: &WorldState,
    ) -> Option<crate::infra::api::cognitive_context::CognitiveContext> {
        use crate::infra::api::cognitive_context::CognitiveContextBuilder;

        // 获取认知上下文构建器（配置的或默认的）
        let builder: &CognitiveContextBuilder = self
            .cognitive_context_builder
            .as_deref()
            .unwrap_or_else(|| {
                // 使用静态默认构建器
                static DEFAULT_BUILDER: std::sync::OnceLock<CognitiveContextBuilder> =
                    std::sync::OnceLock::new();
                DEFAULT_BUILDER.get_or_init(CognitiveContextBuilder::default)
            });

        // 生成认知上下文（不包含人设和关系信息）
        Some(builder.build(world_state))
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 将 WsIntent 转换为 Intent
pub fn ws_intent_to_intent(intent: WsIntent, agent_id: Uuid, tick_id: i64) -> Intent {
    let mut intent_obj = Intent::new(agent_id, tick_id, intent.action_type, intent.action_data);

    if let Some(thought_log) = intent.thought_log {
        intent_obj = intent_obj.with_thought(thought_log);
    }

    intent_obj
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_decision_state_creation() {
        let state = WsDecisionState::new();
        assert_eq!(state.get_current_tick(), 0);
    }

    #[test]
    fn test_set_current_tick() {
        let state = WsDecisionState::new();
        state.set_current_tick(105);
        assert_eq!(state.get_current_tick(), 105);
    }

    #[test]
    fn test_set_tick_duration() {
        let state = WsDecisionState::new();
        state.set_tick_duration(Duration::from_secs(30));
        assert_eq!(state.get_tick_duration(), Duration::from_secs(30));
    }

    #[test]
    fn test_shared_state_from_decision_state() {
        let state = WsDecisionState::new();
        state.set_current_tick(100);

        let shared = WsSharedState::from(&state);
        assert_eq!(shared.get_current_tick(), 100);
    }
}
