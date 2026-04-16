// ============================================================================
// Runtime Module - 运行模式
// ============================================================================
//
// 提供各种决策函数的运行模式实现
//
// ## 设计原则（COI - 组合优于继承）
// - 输入: WorldState
// - 输出: Intent
// - 无副作用
// - 可组合

pub mod claw;

// decision.rs 是单文件模块（原 decision/cognitive.rs）
mod decision;

// 重导出 cognitive
pub use decision::{CognitiveDecisionConfig, cognitive_decision, cognitive_decision_with_chain};
// 重导出 http（已迁移至 infra::api）
pub use crate::infra::api::{
    HttpApiState, HttpDecisionConfig, HttpDecisionState, IntentRequest, create_http_state,
    http_decision, run_http_server,
};
// 重导出 ws（已迁移至 claw/）
pub use claw::{DEFAULT_TICK_DURATION_SECS, WsDecisionState, WsSharedState, ws_router};

use cyber_jianghu_protocol::Intent;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use uuid::Uuid;

// ============================================================================
// 决策函数类型
// ============================================================================

/// 决策回调类型
///
/// 纯函数: (tick_id, agent_id) -> Intent
/// 人魂信息隔离：不传递 WorldState，外部信息由 memory_context 承载
pub type DecisionCallback = Arc<dyn Fn(i64, Uuid) -> BoxFuture<'static, Intent> + Send + Sync>;

/// 带反馈和记忆上下文的决策回调类型
///
/// 接收 tick_id、agent_id、记忆上下文和验证反馈（驳回原因），返回异步 Future
pub type DecisionWithFeedbackCallback =
    Arc<dyn Fn(i64, Uuid, &str, Option<&str>) -> BoxFuture<'static, Intent> + Send + Sync>;

/// 带记忆上下文的决策回调类型
///
/// 接收 tick_id、agent_id 和记忆上下文字符串，返回异步 Future
pub type DecisionWithMemoryCallback =
    Arc<dyn Fn(i64, Uuid, &str) -> BoxFuture<'static, Intent> + Send + Sync>;

// 从 soul::actor 重新导出 CognitiveChain
pub use crate::soul::actor::CognitiveChain;

/// 带 CognitiveChain 的决策回调类型
///
/// 返回 (Intent, CognitiveChain) 元组，
/// 用于三魂架构中天魂翻译时获取人魂的认知上下文辅助指代消解。
pub type DecisionWithChainCallback = Arc<
    dyn Fn(i64, Uuid, &str, Option<&str>) -> BoxFuture<'static, (Intent, Option<CognitiveChain>)>
        + Send
        + Sync,
>;
