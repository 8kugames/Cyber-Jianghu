// ============================================================================
// Decision 模块 - 决策层
// ============================================================================
//
// 纯函数决策，无状态
//
// ## 设计原则（COI - 组合优于继承）
// - 输入: WorldState
// - 输出: Intent
// - 无副作用
// - 可组合
//
// ## 可用模式
// - `http`: HTTP API 服务器（用于 OpenClaw 集成）
// - `cognitive`: 多阶段认知引擎（内置 LLM 决策）

pub mod cognitive;
pub mod http;
pub mod ws;

// 重导出 cognitive
pub use cognitive::{CognitiveDecisionConfig, cognitive_decision, cognitive_decision_with_retry};
// 重导出 http
pub use http::{
    HttpApiState, HttpDecisionConfig, HttpDecisionState, IntentRequest, create_http_state,
    http_decision, run_http_server,
};
// 重导出 ws
pub use ws::{
    WsDecisionConfig, WsDecisionState, WsSharedState, ws_decision, ws_router,
    DEFAULT_TICK_DURATION_SECS, TICK_TIMEOUT_RATIO,
};

use cyber_jianghu_protocol::{Intent, WorldState};
use futures_util::future::BoxFuture;
use std::sync::Arc;

// ============================================================================
// 决策函数类型
// ============================================================================

/// 决策回调类型
///
/// 纯函数: WorldState -> Intent
/// 无状态，可组合
pub type DecisionCallback = Arc<dyn Fn(&WorldState) -> BoxFuture<'static, Intent> + Send + Sync>;

/// 带反馈的决策回调类型
///
/// 接收世界状态和验证反馈（驳回原因），返回异步 Future
pub type DecisionWithFeedbackCallback =
    Arc<dyn Fn(&WorldState, Option<&str>) -> BoxFuture<'static, Intent> + Send + Sync>;

/// 带记忆上下文的决策回调类型
///
/// 接收世界状态和记忆上下文字符串，返回异步 Future
pub type DecisionWithMemoryCallback =
    Arc<dyn Fn(&WorldState, &str) -> BoxFuture<'static, Intent> + Send + Sync>;
