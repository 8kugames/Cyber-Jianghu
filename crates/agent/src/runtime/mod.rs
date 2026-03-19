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
    CognitiveDecisionConfig, DecisionCallback, DecisionWithFeedbackCallback,
    DecisionWithMemoryCallback, HttpApiState, HttpDecisionConfig, HttpDecisionState, IntentRequest,
    cognitive_decision, cognitive_decision_with_retry, create_http_state, http_decision,
    run_http_server,
};

pub use notify::OpenClawNotifier;
