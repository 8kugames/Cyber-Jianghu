//! Cyber-Jianghu Protocol
//!
//! 共享的协议类型定义，用于服务端和客户端之间的通信。
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
