// ============================================================================
// OpenClaw Cyber-Jianghu MVP Tick模块
// ============================================================================
//
// 本模块包含Tick引擎相关的所有功能，包括：
// - Tick循环定时器
// - 状态收集器
// - 状态结算器
// - 状态持久化器
// - 状态广播器
//
// 模块结构：
// - scheduler: Tick调度器（主循环和阶段协调）
// - event_manager: 事件管理器（事件创建和管理）
// - intent_collector: 意图收集器（从IntentManager收集意图）
// - broadcaster: 广播器（向Agent广播WorldState）
// - decay: 生理值衰减计算
// - persistence: 数据库持久化操作
// - state_processor: 状态处理和意图结算
// ============================================================================

mod broadcaster;
mod decay;
mod engine;
mod event_manager;
mod intent_collector;
mod persistence;
mod processor;
mod scheduler;
mod state_processor;

// 导出公共API
pub use broadcaster::{build_initial_world_state, send_to_agent};
pub use scheduler::TickScheduler;
