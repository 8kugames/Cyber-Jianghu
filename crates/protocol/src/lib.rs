//! # Cyber-Jianghu Protocol
//!
//! 定义 Server 和 Agent 之间的通信协议。
//!
//! ## 核心类型
//!
//! - [`ServerMessage`] - 服务端下发的消息
//! - [`ClientMessage`] - 客户端上报的消息
//! - [`WorldState`] - 世界状态快照
//! - [`Intent`] - Agent 意图
//!
//! ## 使用示例
//!
//! ```rust
//! use cyber_jianghu_protocol::{ServerMessage, ClientMessage, Intent};
//! use cyber_jianghu_protocol::ActionType;
//! use uuid::Uuid;
//!
//! // 创建意图
//! let intent = Intent::new(
//!     Uuid::new_v4(),
//!     1,
//!     "说话",
//!     Some(serde_json::json!({"content": "Hello World"})),
//! );
//! ```
//!
//! # Features
//!
//! - `sqlx-support`: 启用 sqlx 数据库类型支持（仅服务端需要）

pub mod error;
pub mod messages;
pub mod types;

// 可选的 sqlx 类型支持
#[cfg(feature = "sqlx-support")]
pub mod sqlx_types;

// 重导出常用类型
pub use messages::{
    ClientMessage, DialogueMessage, DialogueSession, FinalIntentReport, ImmediateIntentReport,
    LayerReport, PipelineAction, RenhunReport, ServerMessage, SoulCycleAttempt,
    SoulCycleMetadata, TianhunReport,
};
pub use types::*;

// 重导出错误类型（从 common 合并）
pub use error::GameError;

/// 协议版本
pub const PROTOCOL_VERSION: &str = env!("CARGO_PKG_VERSION");

// ============================================================================
// 事件类型常量
// ============================================================================

/// 世界状态事件类型
pub const EVENT_TYPE_WORLD_STATE: &str = "world_state";
/// 动作结果事件类型
pub const EVENT_TYPE_ACTION_RESULT: &str = "action_result";
/// 状态变化事件类型
pub const EVENT_TYPE_STATE_CHANGE: &str = "state_change";
/// 公开消息事件类型
pub const EVENT_TYPE_PUBLIC_MESSAGE: &str = "public_message";
/// 系统通知事件类型
pub const EVENT_TYPE_SYSTEM_NOTIFICATION: &str = "system_notification";
/// 死亡通知事件类型
pub const EVENT_TYPE_DEATH_NOTIFICATION: &str = "death_notification";
/// 环境变化事件类型
pub const EVENT_TYPE_ENVIRONMENTAL_CHANGE: &str = "environmental_change";
/// 社交互动事件类型
pub const EVENT_TYPE_SOCIAL_INTERACTION: &str = "social_interaction";

// ============================================================================
// 服务端错误码
// ============================================================================

/// Tick 不匹配（意图的 tick_id 与服务端当前 tick 不一致）
pub const ERROR_CODE_TICK_MISMATCH: &str = "tick_mismatch";
/// 服务端尚未开始接受意图
pub const ERROR_CODE_NOT_ACCEPTING: &str = "not_accepting";
/// Agent 已死亡
pub const ERROR_CODE_AGENT_DEAD: &str = "agent_dead";
/// 速率限制
pub const ERROR_CODE_RATE_LIMITED: &str = "rate_limited";
/// 无效消息格式
pub const ERROR_CODE_INVALID_MESSAGE: &str = "invalid_message";
/// 对话失败
pub const ERROR_CODE_DIALOGUE_FAILED: &str = "dialogue_failed";
/// 动作处理失败（通用）
pub const ERROR_CODE_ACTION_FAILED: &str = "action_failed";
