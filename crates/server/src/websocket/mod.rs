// ============================================================================
// OpenClaw Cyber-Jianghu MVP WebSocket 模块
// ============================================================================
//
// 本模块实现 WebSocket 通信，包括：
// - Agent 连接管理（在线 Agent 列表）
// - 认证机制（基于 auth_token）
// - 意图接收和缓存
// - 状态下发
//
// 设计原则：
// 1. 使用 RwLock 管理连接状态，支持并发读写
// 2. 使用消息队列处理消息（通过 WebSocket 的 split）
// 3. 清晰的错误处理
// 4. 详细的日志记录
//
// 模块结构：
// - types: WebSocket 相关类型定义
// - connection: 连接管理和状态
// - handler: WebSocket 连接处理器
// - broadcast: 消息广播功能
// ============================================================================

mod broadcast;
mod connection;
mod handler;
pub mod types;

// ============================================================================
// Public API 重导出
// ============================================================================

// 类型定义
pub use types::IntentManager;

// 连接管理
pub use connection::{
    AgentToDeviceMap, ConnectionManager, create_agent_to_device_map,
    create_connection_manager, create_intent_manager, take_intents_for_tick,
};

// WebSocket 处理器
pub use handler::websocket_handler;

// 广播功能
pub use broadcast::{
    forward_dialogue_message,
    send_agent_died_notification,
    send_world_state,
};

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    
    use cyber_jianghu_protocol::ServerMessage;

    #[test]
    fn test_server_message_serialization() {
        let msg = ServerMessage::Pong {
            timestamp: 1234567890,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("pong"));

        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::Pong { timestamp } => assert_eq!(timestamp, 1234567890),
            _ => panic!("Unexpected message type"),
        }
    }
}
