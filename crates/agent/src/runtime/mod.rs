// ============================================================================
// Runtime Module - 运行模式
// ============================================================================
//
// 提供各种决策函数的运行模式实现
//
// ## 子模块
// - `decision/` - 各种决策模式（simple, idle, stdio, tcp, http, cognitive）
// - `notify/` - 通知机制

pub mod decision;
pub mod notify;

// 重导出常用的决策类型和函数
pub use decision::{
    DecisionCallback,
    http_decision, HttpDecisionConfig, HttpDecisionState, HttpApiState, create_http_state, run_http_server, IntentRequest,
    cognitive_decision, cognitive_decision_with_retry, CognitiveDecisionConfig,
    DecisionWithFeedbackCallback, DecisionWithMemoryCallback,
};

pub use notify::OpenClawNotifier;
