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
mod lifecycle;
mod reconnect;
mod reflector_ext;
mod social;
pub mod utils;

// 重新导出核心类型
pub use agent::Agent;
pub use builder::AgentBuilder;
pub use reflector_ext::PersonaValidationResult;
pub use crate::soul::reflector::{LayerResult, PipelineValidationResult};

// 从 soul::actor 重导出认知引擎类型
pub use crate::soul::actor::{
    CognitiveChain, CognitiveEngine, CognitiveEngineConfig, CognitiveStage, StageOutput,
};

// 从 runtime 模块重导出决策类型
pub use crate::runtime::{
    DecisionCallback, DecisionWithChainCallback, DecisionWithFeedbackCallback,
    DecisionWithMemoryCallback,
};
