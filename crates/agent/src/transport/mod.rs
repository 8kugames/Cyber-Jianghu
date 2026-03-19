// ============================================================================
// Transport 模块 - 通信层
// ============================================================================
//
// 纯 I/O 操作，负责与服务端通信
// 不包含任何业务逻辑
//
// ## 职责
// - 连接管理（WebSocket）
// - 消息收发（WorldState, Intent）
// - 重连逻辑
//
// ## 不负责
// - 决策逻辑（由 decision 模块负责）
// - 数据验证（由 validator 模块负责）

pub mod websocket;

pub use websocket::{AgentClient, ServerConfig, WebSocketClient};
