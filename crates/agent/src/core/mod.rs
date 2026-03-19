// ============================================================================
// Agent 运行时 - 模块入口
// ============================================================================
//
// 管理 Agent 的主循环：
// 1. 连接服务端
// 2. 接收 WorldState
// 3. 将状态交给外部决策器（如 OpenClaw）
// 4. 发送 Intent
// ============================================================================

mod agent;
mod builder;
pub mod cognitive;
mod lifecycle;
pub mod tools;
pub mod utils;

// 重新导出核心类型
pub use agent::{Agent, PersonaValidationResult, ValidatorConfig};
pub use builder::AgentBuilder;
pub use cognitive::{
    CognitiveChain, CognitiveEngineConfig, CognitiveStage, MultiStageCognitiveEngine, StageOutput,
};

// 从 runtime::decision 模块重导出决策类型
pub use crate::runtime::decision::{
    DecisionCallback, DecisionWithFeedbackCallback, DecisionWithMemoryCallback,
};
