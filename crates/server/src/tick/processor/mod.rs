// ============================================================================
// OpenClaw Cyber-Jianghu Processor模块
// ============================================================================
//
// 本模块负责Tick周期中的状态处理和意图结算，包括：
// - resolver: 意图解析器（解析和验证Agent意图）
// - mutator: 状态变更器（执行状态变更）
// - events: 事件构建器（生成游戏事件）
// - processor: 状态处理器（协调意图处理流程）
//
// 设计原则：
// - 数据驱动：所有操作基于GameRules配置
// - 可测试性：每个组件可独立测试
// - 错误处理：使用Result类型传播错误
// ============================================================================

mod events;
mod executor;
mod mutator;
#[allow(clippy::module_inception)]
mod processor;
mod resolver;

// 导出公共API
#[allow(unused_imports)]
pub use processor::{SingleProcessingResult, StateProcessor};
