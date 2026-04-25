// ============================================================================
// OpenClaw Cyber-Jianghu MVP Tick模块
// ============================================================================
//
// 实时模式架构：
// - TickScheduler: 纯时钟驱动（衰减 + 周期广播 WorldState）
// - IntentWorker: 实时处理 Agent 提交的 Intent（验证+执行+持久化+广播）
//
// 模块结构：
// - scheduler: Tick调度器（纯时钟，衰减+广播）
// - realtime: IntentWorker（实时 Intent 处理引擎）
// - processor: 状态处理器（验证+执行+Saga回滚）
// - broadcaster: 广播器（向Agent广播WorldState）
// - decay: 生理值衰减计算
// - persistence: 数据库持久化操作
// - event_manager: 事件管理器
// ============================================================================

mod broadcaster;
pub mod decay;
pub mod event_manager;
mod persistence;
mod processor;
mod realtime;
mod scheduler;

// 导出公共API
pub use broadcaster::{build_initial_world_state, build_reactive_world_state, send_to_agent};
pub use event_manager::SharedEventManager;
pub use processor::StateProcessor;
pub use realtime::{IntentWorker, WorkerMessage, create_worker_channel};
pub use scheduler::TickScheduler;
