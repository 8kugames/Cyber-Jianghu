// ============================================================================
// OpenClaw Cyber-Jianghu MVP 数据模型
// ============================================================================
//
// 本模块定义了MVP阶段所有核心数据结构，包括：
// - Agent基本信息和状态
// - 物品和背包
// - 意图和动作
// - Tick日志
//
// 设计原则：
// 1. 使用清晰的命名，自解释
// 2. 添加详细的文档注释
// 3. 使用Serde进行序列化/反序列化
// 4. 使用合适的类型（UUID、DateTime等）
// 5. 添加必要的验证（如HP范围0-100）
// 6. 保持简洁，不要过度设计
// ============================================================================

// ============================================================================
// 子模块
// ============================================================================

// Agent 相关
pub mod agent;
pub mod state_creation;
pub mod state_impl;
pub mod state_mutation;

// 物品相关
pub mod items;

// 动作相关
pub mod actions;

// Tick 日志相关
pub mod tick;

// API 响应相关
pub mod responses;

// 验证模块
pub mod validation;

// ============================================================================
// Protocol imports
// ============================================================================

use cyber_jianghu_protocol as protocol;

// ============================================================================
// Protocol types (re-export from cyber_jianghu_protocol)
// ============================================================================

pub use protocol::AgentSelfState;
pub use protocol::AvailableAction;
pub use protocol::Entity;
pub use protocol::GatherableItem;
pub use protocol::InitialItem;
pub use protocol::Intent;
pub use protocol::InventoryItem;
pub use protocol::Location;
pub use protocol::PrivateDialogueRecord;
pub use protocol::RecentAction;
pub use protocol::WorldEvent;
pub use protocol::WorldEventType;
pub use protocol::WorldState;
pub use protocol::WorldTime;

// ============================================================================
// Re-exports from submodules
// ============================================================================

// Agent 相关
pub use agent::{Agent, AgentState};

// 物品相关
pub use items::ItemType;

// 动作相关
pub use actions::{ActionResult, ActionType, AgentAction};

// Tick 相关
pub use tick::TickLog;

// Vendor 待注入事件缓冲区
pub type VendorPendingEvents =
    std::sync::Arc<dashmap::DashMap<uuid::Uuid, Vec<protocol::WorldEvent>>>;

// API 响应相关
pub use responses::{
    AgentRegisterRequest, AgentRegisterResponse, DbHealthStatus, DeviceRegisterErrorResponse,
    DeviceRegisterResponse, DeviceVerifyErrorResponse, DeviceVerifyRequest, DeviceVerifyResponse,
    GameRules, HealthResponse,
};

// ============================================================================
// 验证模块重导出
// ============================================================================

pub use validation::{get_max_agent_name_length, get_max_system_prompt_length};

// ============================================================================
// 测试和示例
// ============================================================================

#[cfg(test)]
mod tests;

#[cfg(test)]
mod jsonb_test;
