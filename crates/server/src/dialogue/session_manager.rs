// ============================================================================
// 对话管理器核心
// ============================================================================
//
// 负责管理对话会话和消息路由
// - 会话状态管理
// - 消息限制检查
// - 会话创建和销毁
//
// ============================================================================

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use cyber_jianghu_protocol::GameError;

use super::session::SessionRegistry;
use super::types::DialogueResponse;

/// 对话管理器
///
/// 管理所有对话会话和消息处理
#[derive(Debug)]
pub struct DialogueManager {
    /// 会话注册表（线程安全）
    sessions: Arc<RwLock<SessionRegistry>>,
    /// 每个 Agent 最大消息数
    max_messages_per_agent: u32,
}

impl DialogueManager {
    /// 创建新的对话管理器
    pub fn new(max_messages_per_agent: u32) -> Self {
        info!("初始化对话管理器 (最大消息数: {})", max_messages_per_agent);

        Self {
            sessions: Arc::new(RwLock::new(SessionRegistry::new())),
            max_messages_per_agent,
        }
    }

    /// 获取会话注册表的可变引用（用于内部模块）
    pub(crate) async fn sessions_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, SessionRegistry> {
        self.sessions.write().await
    }

    /// 获取会话注册表的只读引用（用于测试和内部查询）
    #[cfg(test)]
    pub async fn sessions_read(&self) -> tokio::sync::RwLockReadGuard<'_, SessionRegistry> {
        self.sessions.read().await
    }

    /// 获取最大消息数限制（用于内部模块和测试）
    pub(crate) fn max_messages_per_agent(&self) -> u32 {
        self.max_messages_per_agent
    }

    /// 创建新的对话会话
    ///
    /// 验证双方状态并创建会话
    pub async fn create_session(
        &self,
        from_agent_id: Uuid,
        to_agent_id: Uuid,
    ) -> Result<DialogueResponse, GameError> {
        debug!("收到对话请求: {} -> {}", from_agent_id, to_agent_id);

        let mut registry = self.sessions.write().await;

        // 检查发起方是否已在对话中
        if registry.is_agent_in_dialogue(from_agent_id) {
            warn!("Agent {} 已经在对话中", from_agent_id);
            return Err(GameError::AlreadyInDialogue {
                agent_id: from_agent_id,
            });
        }

        // 检查目标方是否已在对话中
        if registry.is_agent_in_dialogue(to_agent_id) {
            warn!("目标 Agent {} 正在对话中", to_agent_id);
            return Err(GameError::TargetBusy {
                target_agent_id: to_agent_id,
            });
        }

        // 创建新会话
        let session = registry.create_session(from_agent_id, to_agent_id);

        info!("对话会话已创建: {}", session.session_id);

        Ok(DialogueResponse::RequestForwarded {
            session_id: session.session_id,
            target_agent_id: to_agent_id,
        })
    }

    /// 清理超时的对话会话
    ///
    /// timeout: 会话超时时间（从最后活动开始计算）
    /// 返回被清理的会话数量
    pub async fn cleanup_timeout_sessions(&self, timeout: Duration) -> usize {
        let mut registry = self.sessions.write().await;
        registry.cleanup_timeout_sessions(timeout)
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
    async fn test_create_session_success() {
        let manager = create_test_manager();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();

        let result = manager.create_session(agent_a, agent_b).await.unwrap();

        match result {
            DialogueResponse::RequestForwarded {
                session_id,
                target_agent_id,
            } => {
                assert_eq!(target_agent_id, agent_b);
                assert!(!session_id.is_empty());

                // 验证会话已创建
                let registry = manager.sessions_read().await;
                let session = registry.get_session(&session_id);
                assert!(session.is_some());
            }
            _ => panic!("Unexpected response type"),
        }
    }

    #[tokio::test]
    async fn test_create_session_already_in_dialogue() {
        let manager = create_test_manager();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();
        let agent_c = Uuid::new_v4();

        // 创建第一个会话
        manager.create_session(agent_a, agent_b).await.unwrap();

        // 尝试创建第二个会话（应该失败）
        let result = manager.create_session(agent_a, agent_c).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            GameError::AlreadyInDialogue { agent_id } => {
                assert_eq!(agent_id, agent_a);
            }
            _ => panic!("Unexpected error type"),
        }
    }

    #[tokio::test]
    async fn test_create_session_target_busy() {
        let manager = create_test_manager();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();
        let agent_c = Uuid::new_v4();

        // 创建第一个会话
        manager.create_session(agent_a, agent_b).await.unwrap();

        // 尝试与忙碌的目标创建会话（应该失败）
        let result = manager.create_session(agent_c, agent_b).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            GameError::TargetBusy { target_agent_id } => {
                assert_eq!(target_agent_id, agent_b);
            }
            _ => panic!("Unexpected error type"),
        }
    }
}
