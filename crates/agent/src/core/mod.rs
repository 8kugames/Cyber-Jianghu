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

// ============================================================================
// 运行时常量
// ============================================================================

/// 遗忘机制运行间隔（tick 数）
///
/// 基于艾宾浩斯遗忘曲线，每隔一定 tick 运行遗忘检查。
///
/// NOTE: 当前 84 tick 是基于默认 tick_duration=180s 的经验值。
/// 理想情况下应基于游戏时间（而非 tick 数）配置，例如每 7 个游戏日运行一次。
/// 这样在不同 tick_duration 设置下都能保持一致的游戏体验。
///
/// 未来改进：将配置改为游戏时间间隔（如 `forgetting_interval_game_days: 7`），
/// 在运行时根据 tick_duration 和 time.yaml 配置计算出实际 tick 数。
pub const FORGETTING_INTERVAL_TICKS: i64 = 84;

// 重新导出核心类型
pub use agent::Agent;
pub use builder::AgentBuilder;
pub use reflector_ext::{LayerResult, PersonaValidationResult, ReflectorResult};

// 从 soul::actor 重导出认知引擎类型
pub use crate::soul::actor::{
    CognitiveChain, CognitiveEngine, CognitiveEngineConfig, CognitiveStage, StageOutput,
};

// 从 runtime 模块重导出决策类型
pub use crate::runtime::{
    DecisionCallback, DecisionWithChainCallback, DecisionWithFeedbackCallback,
    DecisionWithMemoryCallback,
};
