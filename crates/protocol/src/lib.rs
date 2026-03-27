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
//!     "speak",
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
pub use messages::{ClientMessage, DialogueMessage, DialogueSession, ServerMessage};
pub use types::*;

// 重导出错误类型（从 common 合并）
pub use error::GameError;

/// 协议版本
pub const PROTOCOL_VERSION: &str = env!("CARGO_PKG_VERSION");
