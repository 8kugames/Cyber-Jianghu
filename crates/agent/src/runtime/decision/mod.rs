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
// ## Agent 架构说明
//
// Agent 仅支持 Claw 模式，外部调度器（如 OpenClaw）通过 WebSocket + HTTP API
// 与 Agent 通信。Agent 不内置 LLM 调用，LLM 决策由外部调度器负责。
//
// ### 通信协议
// - WebSocket `/ws`: OpenClaw 连接，Agent 推送 Tick，OpenClaw 提交 Intent
// - HTTP `/api/v1/*`: 数据查询接口（状态、属性、记忆等）
//
// ### 超时处理
// - Tick 截止时间到达时，OpenClaw 未提交 Intent → Agent 自动提交 idle Intent
//
// ### Cognitive 阶段参考
// - 四阶段认知框架（Perception → Motivation → Planning → Decision）定义在
//   `core/cognitive/stages.rs`，作为 OpenClaw 实现者的参考文档
// - Agent 通过 `ai/cognitive/narrative.rs` 生成叙事化上下文，引导 OpenClaw 的 LLM 推理

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
    DEFAULT_TICK_DURATION_SECS, TICK_TIMEOUT_RATIO, WsDecisionConfig, WsDecisionState,
    WsSharedState, ws_decision, ws_router,
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
