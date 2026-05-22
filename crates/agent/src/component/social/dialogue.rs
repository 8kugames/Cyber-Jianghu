//! 对话客户端
//!
//! 提供 Agent 间对话功能的客户端接口。

use cyber_jianghu_protocol::DialogueMessage;
use std::sync::Arc;
use uuid::Uuid;

// ============================================================================
// 对话事件处理器 Trait
// ============================================================================

/// 对话事件处理器
///
/// Agent 需要实现此 trait 以处理对话相关事件。
/// 所有方法都是可选的，Agent 可以根据需要选择实现。
pub trait DialogueEventHandler: Send + Sync {
    /// 收到对话请求
    fn on_dialogue_request(&self, from_agent_id: Uuid, opening_remark: String) {
        let _ = (from_agent_id, opening_remark);
    }

    /// 对话请求被接受
    fn on_dialogue_accepted(&self, session_id: String) {
        let _ = session_id;
    }

    /// 对话请求被拒绝
    fn on_dialogue_rejected(&self, session_id: String, reason: Option<String>) {
        let _ = (session_id, reason);
    }

    /// 收到对话消息
    fn on_dialogue_message(&self, session_id: String, from_agent_id: Uuid, content: String) {
        let _ = (session_id, from_agent_id, content);
    }

    /// 对话结束
    fn on_dialogue_ended(&self, session_id: String, by_agent: Uuid) {
        let _ = (session_id, by_agent);
    }
}

// ============================================================================
// 对话客户端
// ============================================================================

/// 对话客户端
///
/// 管理 Agent 的对话功能，提供发送和接收对话消息的接口。
///
/// # 示例
///
/// ```rust
/// use cyber_jianghu_agent::component::social::{DialogueClient, DialogueEventHandler};
/// use uuid::Uuid;
/// use std::sync::Arc;
///
/// struct MyHandler;
///
/// impl DialogueEventHandler for MyHandler {
///     fn on_dialogue_request(&self, from_agent_id: Uuid, opening_remark: String) {
///         println!("收到来自 {} 的对话请求: {}", from_agent_id, opening_remark);
///     }
/// }
///
/// let agent_id = Uuid::new_v4();
/// let handler = Arc::new(MyHandler);
/// let client = DialogueClient::new(agent_id, handler);
/// let target_agent_id = Uuid::new_v4();
///
/// // 请求对话
/// let message = client.request_dialogue(target_agent_id, "你好".to_string());
/// // 将 message 通过 WebSocket 发送
/// ```
pub struct DialogueClient {
    /// 当前 Agent ID
    agent_id: Uuid,
    /// 事件处理器
    handler: Arc<dyn DialogueEventHandler>,
}

impl Clone for DialogueClient {
    fn clone(&self) -> Self {
        Self {
            agent_id: self.agent_id,
            handler: Arc::clone(&self.handler),
        }
    }
}

impl DialogueClient {
    /// 创建新的对话客户端
    ///
    /// # 参数
    ///
    /// - `agent_id`: 当前 Agent 的 ID
    /// - `handler`: 事件处理器，用于处理收到的对话事件
    ///
    /// # 示例
    ///
    /// ```rust
    /// # use cyber_jianghu_agent::component::social::{DialogueClient, DialogueEventHandler};
    /// # use uuid::Uuid;
    /// # use std::sync::Arc;
    /// # struct MyHandler;
    /// # impl DialogueEventHandler for MyHandler {}
    /// # let agent_id = Uuid::new_v4();
    /// # let client = DialogueClient::new(agent_id, Arc::new(MyHandler));
    /// # let target_agent_id = Uuid::new_v4();
    /// let message = client.request_dialogue(target_agent_id, "你好".to_string());
    /// ```
    pub fn new(agent_id: Uuid, handler: Arc<dyn DialogueEventHandler>) -> Self {
        Self { agent_id, handler }
    }

    /// 请求与另一个 Agent 建立对话
    ///
    /// # 参数
    ///
    /// - `to_agent_id`: 目标 Agent 的 ID
    /// - `opening_remark`: 开场白
    ///
    /// # 返回
    ///
    /// 返回一个 `DialogueMessage::Request`，需要通过 WebSocket 发送给服务端。
    ///
    /// # 示例
    ///
    /// ```rust
    /// # use cyber_jianghu_agent::component::social::{DialogueClient, DialogueEventHandler};
    /// # use uuid::Uuid;
    /// # use std::sync::Arc;
    /// # struct MyHandler;
    /// # impl DialogueEventHandler for MyHandler {}
    /// # let agent_id = Uuid::new_v4();
    /// # let client = DialogueClient::new(agent_id, Arc::new(MyHandler));
    /// # let target_id = Uuid::new_v4();
    /// let message = client.request_dialogue(target_id, "你好，能聊聊吗？".to_string());
    /// // 将 message 序列化并通过 WebSocket 发送
    /// ```
    pub fn request_dialogue(&self, to_agent_id: Uuid, opening_remark: String) -> DialogueMessage {
        DialogueMessage::Request {
            from_agent_id: self.agent_id,
            to_agent_id,
            opening_remark,
        }
    }

    /// 接受对话请求
    ///
    /// # 参数
    ///
    /// - `session_id`: 会话 ID
    ///
    /// # 返回
    ///
    /// 返回一个 `DialogueMessage::Accept`，需要通过 WebSocket 发送给服务端。
    pub fn accept_dialogue(&self, session_id: String) -> DialogueMessage {
        DialogueMessage::Accept {
            session_id,
            from_agent_id: self.agent_id,
        }
    }

    /// 拒绝对话请求
    ///
    /// # 参数
    ///
    /// - `session_id`: 会话 ID
    /// - `reason`: 拒绝原因（可选）
    ///
    /// # 返回
    ///
    /// 返回一个 `DialogueMessage::Reject`，需要通过 WebSocket 发送给服务端。
    pub fn reject_dialogue(&self, session_id: String, reason: Option<String>) -> DialogueMessage {
        DialogueMessage::Reject {
            session_id,
            from_agent_id: self.agent_id,
            reason,
        }
    }

    /// 发送对话消息
    ///
    /// # 参数
    ///
    /// - `session_id`: 会话 ID
    /// - `content`: 消息内容
    ///
    /// # 返回
    ///
    /// 返回一个 `DialogueMessage::Content`，需要通过 WebSocket 发送给服务端。
    pub fn send_message(&self, session_id: String, content: String) -> DialogueMessage {
        DialogueMessage::Content {
            session_id,
            from_agent_id: self.agent_id,
            content,
        }
    }

    /// 结束对话
    ///
    /// # 参数
    ///
    /// - `session_id`: 会话 ID
    ///
    /// # 返回
    ///
    /// 返回一个 `DialogueMessage::End`，需要通过 WebSocket 发送给服务端。
    pub fn end_dialogue(&self, session_id: String) -> DialogueMessage {
        DialogueMessage::End {
            session_id,
            from_agent_id: self.agent_id,
        }
    }

    /// 处理收到的对话消息
    ///
    /// 此方法会解析消息并调用相应的事件处理器方法。
    ///
    /// # 参数
    ///
    /// - `message`: 收到的对话消息
    ///
    /// # 示例
    ///
    /// ```rust
    /// # use cyber_jianghu_agent::component::social::{DialogueClient, DialogueEventHandler};
    /// # use cyber_jianghu_protocol::ServerMessage;
    /// # use cyber_jianghu_protocol::DialogueMessage;
    /// # use uuid::Uuid;
    /// # use std::sync::Arc;
    /// # struct MyHandler;
    /// # impl DialogueEventHandler for MyHandler {}
    /// # let agent_id = Uuid::new_v4();
    /// # let dialogue_client = DialogueClient::new(agent_id, Arc::new(MyHandler));
    /// # let target_id = Uuid::new_v4();
    /// # let message = DialogueMessage::Request {
    /// #     from_agent_id: target_id,
    /// #     to_agent_id: agent_id,
    /// #     opening_remark: "Hi".to_string()
    /// # };
    /// # let server_message = ServerMessage::Dialogue { message };
    /// // 在收到 WebSocket 消息后
    /// if let ServerMessage::Dialogue { message } = server_message {
    ///     dialogue_client.handle_message(message);
    /// }
    /// ```
    pub fn handle_message(&self, message: DialogueMessage) {
        match message {
            DialogueMessage::Request {
                from_agent_id,
                opening_remark,
                ..
            } => {
                self.handler
                    .on_dialogue_request(from_agent_id, opening_remark);
            }
            DialogueMessage::Accept { session_id, .. } => {
                self.handler.on_dialogue_accepted(session_id);
            }
            DialogueMessage::Reject {
                session_id, reason, ..
            } => {
                self.handler.on_dialogue_rejected(session_id, reason);
            }
            DialogueMessage::Content {
                session_id,
                from_agent_id,
                content,
            } => {
                self.handler
                    .on_dialogue_message(session_id, from_agent_id, content);
            }
            DialogueMessage::End {
                session_id,
                from_agent_id,
            } => {
                self.handler.on_dialogue_ended(session_id, from_agent_id);
            }
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestHandler {
        requests_received: Mutex<Vec<(Uuid, String)>>,
        accepted: Mutex<Vec<String>>,
        rejected: Mutex<Vec<(String, Option<String>)>>,
        messages: Mutex<Vec<(String, Uuid, String)>>,
        ended: Mutex<Vec<(String, Uuid)>>,
    }

    impl DialogueEventHandler for TestHandler {
        fn on_dialogue_request(&self, from_agent_id: Uuid, opening_remark: String) {
            self.requests_received
                .lock()
                .unwrap()
                .push((from_agent_id, opening_remark));
        }

        fn on_dialogue_accepted(&self, session_id: String) {
            self.accepted.lock().expect("lock poisoned").push(session_id);
        }

        fn on_dialogue_rejected(&self, session_id: String, reason: Option<String>) {
            self.rejected.lock().expect("lock poisoned").push((session_id, reason));
        }

        fn on_dialogue_message(&self, session_id: String, from_agent_id: Uuid, content: String) {
            self.messages
                .lock()
                .unwrap()
                .push((session_id, from_agent_id, content));
        }

        fn on_dialogue_ended(&self, session_id: String, by_agent: Uuid) {
            self.ended.lock().expect("lock poisoned").push((session_id, by_agent));
        }
    }

    #[test]
    fn test_request_dialogue() {
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let handler: Arc<dyn DialogueEventHandler> = Arc::new(TestHandler::default());
        let client = DialogueClient::new(agent_id, handler);

        let message = client.request_dialogue(target_id, "你好".to_string());

        match message {
            DialogueMessage::Request {
                from_agent_id,
                to_agent_id,
                opening_remark,
            } => {
                assert_eq!(from_agent_id, agent_id);
                assert_eq!(to_agent_id, target_id);
                assert_eq!(opening_remark, "你好");
            }
            _ => panic!("Expected Request message"),
        }
    }

    #[test]
    fn test_accept_dialogue() {
        let agent_id = Uuid::new_v4();
        let handler: Arc<dyn DialogueEventHandler> = Arc::new(TestHandler::default());
        let client = DialogueClient::new(agent_id, handler);
        let session_id = "test-session".to_string();

        let message = client.accept_dialogue(session_id.clone());

        match message {
            DialogueMessage::Accept {
                session_id: sid,
                from_agent_id,
            } => {
                assert_eq!(sid, session_id);
                assert_eq!(from_agent_id, agent_id);
            }
            _ => panic!("Expected Accept message"),
        }
    }

    #[test]
    fn test_reject_dialogue() {
        let agent_id = Uuid::new_v4();
        let handler: Arc<dyn DialogueEventHandler> = Arc::new(TestHandler::default());
        let client = DialogueClient::new(agent_id, handler);
        let session_id = "test-session".to_string();

        let message = client.reject_dialogue(session_id.clone(), Some("忙".to_string()));

        match message {
            DialogueMessage::Reject {
                session_id: sid,
                from_agent_id,
                reason,
            } => {
                assert_eq!(sid, session_id);
                assert_eq!(from_agent_id, agent_id);
                assert_eq!(reason, Some("忙".to_string()));
            }
            _ => panic!("Expected Reject message"),
        }
    }

    #[test]
    fn test_send_message() {
        let agent_id = Uuid::new_v4();
        let handler: Arc<dyn DialogueEventHandler> = Arc::new(TestHandler::default());
        let client = DialogueClient::new(agent_id, handler);
        let session_id = "test-session".to_string();

        let message = client.send_message(session_id.clone(), "你好".to_string());

        match message {
            DialogueMessage::Content {
                session_id: sid,
                from_agent_id,
                content,
            } => {
                assert_eq!(sid, session_id);
                assert_eq!(from_agent_id, agent_id);
                assert_eq!(content, "你好");
            }
            _ => panic!("Expected Content message"),
        }
    }

    #[test]
    fn test_end_dialogue() {
        let agent_id = Uuid::new_v4();
        let handler: Arc<dyn DialogueEventHandler> = Arc::new(TestHandler::default());
        let client = DialogueClient::new(agent_id, handler);
        let session_id = "test-session".to_string();

        let message = client.end_dialogue(session_id.clone());

        match message {
            DialogueMessage::End {
                session_id: sid,
                from_agent_id,
            } => {
                assert_eq!(sid, session_id);
                assert_eq!(from_agent_id, agent_id);
            }
            _ => panic!("Expected End message"),
        }
    }

    #[test]
    fn test_handle_request_message() {
        let agent_id = Uuid::new_v4();
        let from_id = Uuid::new_v4();
        let handler = Arc::new(TestHandler::default());
        let client = DialogueClient::new(agent_id, handler.clone());

        let message = DialogueMessage::Request {
            from_agent_id: from_id,
            to_agent_id: agent_id,
            opening_remark: "你好".to_string(),
        };

        client.handle_message(message);

        let requests = handler.requests_received.lock().expect("lock poisoned");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, from_id);
        assert_eq!(requests[0].1, "你好");
    }

    #[test]
    fn test_handle_content_message() {
        let agent_id = Uuid::new_v4();
        let from_id = Uuid::new_v4();
        let handler = Arc::new(TestHandler::default());
        let client = DialogueClient::new(agent_id, handler.clone());

        let message = DialogueMessage::Content {
            session_id: "test-session".to_string(),
            from_agent_id: from_id,
            content: "在吗".to_string(),
        };

        client.handle_message(message);

        let messages = handler.messages.lock().expect("lock poisoned");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, "test-session");
        assert_eq!(messages[0].1, from_id);
        assert_eq!(messages[0].2, "在吗");
    }
}
