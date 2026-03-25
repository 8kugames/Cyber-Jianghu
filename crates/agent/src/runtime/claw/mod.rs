// Claw Runtime - 内部调度器运行时
//
// 提供替代 OpenClaw 的轻量级调度能力：
// - History Manager：防止 context overflow
// - Turn Cycle：Agent 决策循环
// - Context Builder：构建 LLM 调用上下文
//
// 核心原则：
// 1. 极简：只实现必要的功能
// 2. Fail Fast：不允许静默失败
// 3. 自控：完全控制 context 管理

mod history;
mod turn_cycle;

pub use history::{
    ChatMessage, HistoryConfig, HistoryEntry, HistoryHealth, HistoryManager, HealthStatus,
};
pub use turn_cycle::{
    Intent, ToolCall, ToolResult, TurnCycle, TurnCycleConfig, TurnCycleServices,
};
