// ============================================================================
// 对话消息处理器
// ============================================================================
//
// 负责处理各种类型的对话消息
// - Request/Accept/Reject 处理
// - Content 消息转发
// - End 会话结束
//
// ============================================================================

use tracing::{debug, info, warn};
use uuid::Uuid;

use cyber_jianghu_protocol::DialogueMessage;
use cyber_jianghu_protocol::GameError;

use super::session::SessionStatus;
use super::session_manager::DialogueManager;
use super::types::DialogueResponse;

impl DialogueManager {
    /// 处理对话消息
    ///
    /// 根据 DialogueMessage 类型分发到对应的处理方法
    pub async fn handle_message(
        &self,
        message: DialogueMessage,
    ) -> Result<DialogueResponse, GameError> {
        match message {
            DialogueMessage::Request {
                from_agent_id,
                to_agent_id,
                opening_remark: _,
            } => self.create_session(from_agent_id, to_agent_id).await,

            DialogueMessage::Accept {
                session_id,
                from_agent_id,
            } => self.handle_accept(session_id, from_agent_id).await,

            DialogueMessage::Reject {
                session_id,
                from_agent_id,
                reason: _,
            } => self.handle_reject(session_id, from_agent_id).await,

            DialogueMessage::Content {
                session_id,
                from_agent_id,
                content: _,
            } => self.handle_content(session_id, from_agent_id).await,

            DialogueMessage::End {
                session_id,
                from_agent_id,
            } => self.handle_end(session_id, from_agent_id).await,
        }
    }

    /// 处理接受对话
    ///
    /// 激活会话
    pub async fn handle_accept(
        &self,
        session_id: String,
        from_agent_id: Uuid,
    ) -> Result<DialogueResponse, GameError> {
        debug!(
            "接受对话: session_id={}, agent={}",
            session_id, from_agent_id
        );

        let mut registry = self.sessions_mut().await;

        // 获取会话
        let session = registry.get_session(&session_id).ok_or_else(|| {
            warn!("会话不存在: {}", session_id);
            GameError::SessionNotFound {
                session_id: session_id.clone(),
            }
        })?;

        // 验证参与者
        if !session.involves(from_agent_id) {
            warn!("Agent {} 不是会话 {} 的参与者", from_agent_id, session_id);
            return Err(GameError::NotParticipant {
                agent_id: from_agent_id,
                session_id,
            });
        }

        // 保存需要的字段
        let agent_a = session.agent_a;
        let agent_b = session.agent_b;

        // 更新会话状态为 Active
        let mut updated = session.clone();
        updated.status = SessionStatus::Active;
        registry.update_session(&session_id, updated);

        info!("对话会话已激活: {}", session_id);

        Ok(DialogueResponse::SessionStarted {
            session_id,
            agent_a,
            agent_b,
        })
    }

    /// 处理拒绝对话
    ///
    /// 移除会话
    pub async fn handle_reject(
        &self,
        session_id: String,
        from_agent_id: Uuid,
    ) -> Result<DialogueResponse, GameError> {
        debug!(
            "拒绝对话: session_id={}, agent={}",
            session_id, from_agent_id
        );

        let mut registry = self.sessions_mut().await;

        // 获取会话
        let session = registry.get_session(&session_id).ok_or_else(|| {
            warn!("会话不存在: {}", session_id);
            GameError::SessionNotFound {
                session_id: session_id.clone(),
            }
        })?;

        // 验证参与者
        if !session.involves(from_agent_id) {
            warn!("Agent {} 不是会话 {} 的参与者", from_agent_id, session_id);
            return Err(GameError::NotParticipant {
                agent_id: from_agent_id,
                session_id,
            });
        }

        // 在移除会话之前保存双方 agent ID
        let requester = session.agent_a;
        let rejected_by = session.agent_b;

        // 移除会话
        registry.remove_session(&session_id);

        info!("对话会话已拒绝并移除: {}", session_id);

        Ok(DialogueResponse::SessionRejected {
            session_id,
            rejected_by,
            requester,
        })
    }

    /// 处理对话内容
    ///
    /// 转发消息并更新计数
    pub async fn handle_content(
        &self,
        session_id: String,
        from_agent_id: Uuid,
    ) -> Result<DialogueResponse, GameError> {
        debug!(
            "对话内容: session_id={}, agent={}",
            session_id, from_agent_id
        );

        let mut registry = self.sessions_mut().await;

        // 获取会话
        let session = registry.get_session(&session_id).ok_or_else(|| {
            warn!("会话不存在: {}", session_id);
            GameError::SessionNotFound {
                session_id: session_id.clone(),
            }
        })?;

        // 验证参与者
        if !session.involves(from_agent_id) {
            warn!("Agent {} 不是会话 {} 的参与者", from_agent_id, session_id);
            return Err(GameError::NotParticipant {
                agent_id: from_agent_id,
                session_id,
            });
        }

        // 检查会话状态
        if session.status != SessionStatus::Active {
            warn!("会话 {} 未激活", session_id);
            return Err(GameError::SessionNotActive { session_id });
        }

        // 检查消息限制
        if session.is_message_limit_reached(self.max_messages_per_agent()) {
            warn!("会话 {} 消息数量已达上限", session_id);
            return Err(GameError::MessageLimitReached { session_id });
        }

        // 获取目标 Agent（在更新会话之前）
        let to_agent_id = session.get_partner(from_agent_id).ok_or_else(|| {
            warn!("Agent {} 的伙伴不存在", from_agent_id);
            GameError::NotParticipant {
                agent_id: from_agent_id,
                session_id: session_id.clone(),
            }
        })?;

        // 更新消息计数
        let mut updated = session.clone();
        updated.increment_message_count();
        registry.update_session(&session_id, updated);

        debug!("消息已转发: {} -> {}", from_agent_id, to_agent_id);

        Ok(DialogueResponse::ContentForward {
            session_id,
            from_agent_id,
            to_agent_id,
        })
    }

    /// 处理结束对话
    ///
    /// 移除会话
    pub async fn handle_end(
        &self,
        session_id: String,
        from_agent_id: Uuid,
    ) -> Result<DialogueResponse, GameError> {
        debug!(
            "结束对话: session_id={}, agent={}",
            session_id, from_agent_id
        );

        let mut registry = self.sessions_mut().await;

        // 获取会话
        let session = registry.get_session(&session_id).ok_or_else(|| {
            warn!("会话不存在: {}", session_id);
            GameError::SessionNotFound {
                session_id: session_id.clone(),
            }
        })?;

        // 验证参与者
        if !session.involves(from_agent_id) {
            warn!("Agent {} 不是会话 {} 的参与者", from_agent_id, session_id);
            return Err(GameError::NotParticipant {
                agent_id: from_agent_id,
                session_id,
            });
        }

        // 在移除会话之前获取双方 agent ID
        let other_participant = session.get_partner(from_agent_id).ok_or_else(|| {
            warn!("Agent {} 的伙伴不存在", from_agent_id);
            GameError::NotParticipant {
                agent_id: from_agent_id,
                session_id: session_id.clone(),
            }
        })?;

        // 移除会话
        registry.remove_session(&session_id);

        info!("对话会话已结束: {}", session_id);

        Ok(DialogueResponse::SessionEnded {
            session_id,
            ended_by: from_agent_id,
            other_participant,
        })
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_manager() -> DialogueManager {
        DialogueManager::new(10)
    }

    #[tokio::test]
    async fn test_handle_accept_success() {
        let manager = create_test_manager();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();

        // 创建会话
        let request_result = manager.create_session(agent_a, agent_b).await.unwrap();
        let session_id = match request_result {
            DialogueResponse::RequestForwarded { session_id, .. } => session_id,
            _ => panic!("Unexpected response type"),
        };

        // 接受会话
        let result = manager.handle_accept(session_id.clone(), agent_b).await;

        assert!(result.is_ok());
        match result.unwrap() {
            DialogueResponse::SessionStarted {
                session_id: returned_id,
                agent_a: returned_a,
                agent_b: returned_b,
            } => {
                assert_eq!(returned_id, session_id);
                assert_eq!(returned_a, agent_a);
                assert_eq!(returned_b, agent_b);

                // 验证会话状态
                let registry = manager.sessions_read().await;
                let session = registry.get_session(&session_id);
                assert!(session.is_some());
                assert_eq!(session.unwrap().status, SessionStatus::Active);
            }
            _ => panic!("Unexpected response type"),
        }
    }

    #[tokio::test]
    async fn test_handle_reject_success() {
        let manager = create_test_manager();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();

        // 创建会话
        let request_result = manager.create_session(agent_a, agent_b).await.unwrap();
        let session_id = match request_result {
            DialogueResponse::RequestForwarded { session_id, .. } => session_id,
            _ => panic!("Unexpected response type"),
        };

        // 拒绝会话
        let result = manager.handle_reject(session_id.clone(), agent_b).await;

        assert!(result.is_ok());
        match result.unwrap() {
            DialogueResponse::SessionRejected {
                session_id: returned_id,
                rejected_by,
                requester,
            } => {
                assert_eq!(returned_id, session_id);
                assert_eq!(rejected_by, agent_b);
                assert_eq!(requester, agent_a);
            }
            _ => panic!("Unexpected response type"),
        }

        // 验证会话已移除
        let registry = manager.sessions_read().await;
        let session = registry.get_session(&session_id);
        assert!(session.is_none());
    }

    #[tokio::test]
    async fn test_handle_content_success() {
        let manager = create_test_manager();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();

        // 创建并激活会话
        let request_result = manager.create_session(agent_a, agent_b).await.unwrap();
        let session_id = match request_result {
            DialogueResponse::RequestForwarded { session_id, .. } => session_id,
            _ => panic!("Unexpected response type"),
        };
        manager
            .handle_accept(session_id.clone(), agent_b)
            .await
            .unwrap();

        // 发送消息
        let result = manager.handle_content(session_id.clone(), agent_a).await;

        assert!(result.is_ok());
        match result.unwrap() {
            DialogueResponse::ContentForward {
                session_id: returned_id,
                from_agent_id,
                to_agent_id,
            } => {
                assert_eq!(returned_id, session_id);
                assert_eq!(from_agent_id, agent_a);
                assert_eq!(to_agent_id, agent_b);

                // 验证消息计数
                let registry = manager.sessions_read().await;
                let session = registry.get_session(&session_id);
                assert!(session.is_some());
                assert_eq!(session.unwrap().message_count, 1);
            }
            _ => panic!("Unexpected response type"),
        }
    }

    #[tokio::test]
    async fn test_handle_end_success() {
        let manager = create_test_manager();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();

        // 创建并激活会话
        let request_result = manager.create_session(agent_a, agent_b).await.unwrap();
        let session_id = match request_result {
            DialogueResponse::RequestForwarded { session_id, .. } => session_id,
            _ => panic!("Unexpected response type"),
        };
        manager
            .handle_accept(session_id.clone(), agent_b)
            .await
            .unwrap();

        // 结束会话
        let result = manager.handle_end(session_id.clone(), agent_a).await;

        assert!(result.is_ok());
        match result.unwrap() {
            DialogueResponse::SessionEnded {
                session_id: returned_id,
                ended_by,
                other_participant,
            } => {
                assert_eq!(returned_id, session_id);
                assert_eq!(ended_by, agent_a);
                assert_eq!(other_participant, agent_b);
            }
            _ => panic!("Unexpected response type"),
        }

        // 验证会话已移除
        let registry = manager.sessions_read().await;
        let session = registry.get_session(&session_id);
        assert!(session.is_none());
    }

    #[tokio::test]
    async fn test_message_limit() {
        let manager = create_test_manager();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();

        // 创建并激活会话
        let request_result = manager.create_session(agent_a, agent_b).await.unwrap();
        let session_id = match request_result {
            DialogueResponse::RequestForwarded { session_id, .. } => session_id,
            _ => panic!("Unexpected response type"),
        };
        manager
            .handle_accept(session_id.clone(), agent_b)
            .await
            .unwrap();

        // 发送消息直到达到上限
        for _ in 0..20 {
            manager
                .handle_content(session_id.clone(), agent_a)
                .await
                .unwrap();
        }

        // 下一条消息应该失败
        let result = manager.handle_content(session_id.clone(), agent_a).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            GameError::MessageLimitReached { session_id: id } => {
                assert_eq!(id, session_id);
            }
            _ => panic!("Unexpected error type"),
        }
    }
}
