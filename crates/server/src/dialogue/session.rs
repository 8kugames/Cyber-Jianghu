// ============================================================================
// 对话会话管理
// ============================================================================
//
// 负责存储和管理 Agent 之间的对话会话
//
// ============================================================================

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use uuid::Uuid;

/// 对话会话状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// 待接受（已发起请求，等待对方响应）
    Pending,
    /// 活跃中（双方已建立连接）
    Active,
}

/// 对话会话
///
/// 记录两个 Agent 之间的对话状态
#[derive(Debug, Clone)]
pub struct DialogueSession {
    /// 会话唯一标识
    pub session_id: String,
    /// 发起方 Agent ID
    pub agent_a: Uuid,
    /// 接收方 Agent ID
    pub agent_b: Uuid,
    /// 消息计数
    pub message_count: u32,
    /// 会话状态
    pub status: SessionStatus,
    /// 创建时间（预留：会话时长统计）
    #[allow(dead_code)]
    pub created_at: Instant,
    /// 最后活动时间
    pub last_activity_at: Instant,
}

impl DialogueSession {
    /// 创建新的对话会话
    pub fn new(agent_a: Uuid, agent_b: Uuid) -> Self {
        let now = Instant::now();
        Self {
            session_id: Uuid::new_v4().to_string(),
            agent_a,
            agent_b,
            message_count: 0,
            status: SessionStatus::Pending,
            created_at: now,
            last_activity_at: now,
        }
    }

    /// 检查指定 Agent 是否参与此会话
    pub fn involves(&self, agent_id: Uuid) -> bool {
        self.agent_a == agent_id || self.agent_b == agent_id
    }

    /// 获取对话伙伴的 ID
    ///
    /// 如果 agent_id 是 agent_a，返回 agent_b，反之亦然
    pub fn get_partner(&self, agent_id: Uuid) -> Option<Uuid> {
        if self.agent_a == agent_id {
            Some(self.agent_b)
        } else if self.agent_b == agent_id {
            Some(self.agent_a)
        } else {
            None
        }
    }

    /// 增加消息计数并更新活动时间
    pub fn increment_message_count(&mut self) {
        self.message_count += 1;
        self.last_activity_at = Instant::now();
    }

    /// 检查是否达到消息限制
    ///
    /// max_messages 是每个 Agent 的最大消息数
    /// 总消息限制 = max_messages * 2（两个 Agent）
    pub fn is_message_limit_reached(&self, max_messages: u32) -> bool {
        self.message_count >= max_messages * 2
    }

    /// 检查会话是否已超时
    ///
    /// timeout: 会话超时时间（从最后活动开始计算）
    #[allow(dead_code)]
    pub fn is_timeout(&self, timeout: Duration) -> bool {
        self.last_activity_at.elapsed() > timeout
    }
}

/// 会话注册表
///
/// 存储所有活跃的对话会话
#[derive(Debug)]
pub struct SessionRegistry {
    /// 会话存储：session_id -> DialogueSession
    sessions: HashMap<String, DialogueSession>,
    /// Agent 索引：agent_id -> session_id（用于快速查找 Agent 当前所在的会话）
    agent_index: HashMap<Uuid, String>,
}

impl SessionRegistry {
    /// 创建新的会话注册表
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            agent_index: HashMap::new(),
        }
    }

    /// 创建新的对话会话
    ///
    /// 返回新创建的会话
    pub fn create_session(&mut self, agent_a: Uuid, agent_b: Uuid) -> DialogueSession {
        let session = DialogueSession::new(agent_a, agent_b);
        let session_id = session.session_id.clone();

        info!(
            "创建对话会话: {} <-> {} (session_id: {})",
            agent_a, agent_b, session_id
        );

        self.sessions.insert(session_id.clone(), session.clone());
        self.agent_index.insert(agent_a, session_id.clone());
        self.agent_index.insert(agent_b, session_id);

        session
    }

    /// 获取指定会话
    pub fn get_session(&self, session_id: &str) -> Option<&DialogueSession> {
        self.sessions.get(session_id)
    }

    /// 获取指定 Agent 当前所在的会话
    #[allow(dead_code)]
    pub fn get_agent_session(&self, agent_id: Uuid) -> Option<&DialogueSession> {
        self.agent_index
            .get(&agent_id)
            .and_then(|session_id| self.sessions.get(session_id))
    }

    /// 更新会话状态
    ///
    /// 返回更新前的会话状态，如果会话不存在则返回 None
    pub fn update_session(
        &mut self,
        session_id: &str,
        session: DialogueSession,
    ) -> Option<DialogueSession> {
        info!(
            "更新会话状态: {} (status: {:?})",
            session_id, session.status
        );
        self.sessions
            .insert(session_id.to_string(), session.clone())
    }

    /// 移除会话
    ///
    /// 返回被移除的会话，如果会话不存在则返回 None
    pub fn remove_session(&mut self, session_id: &str) -> Option<DialogueSession> {
        if let Some(session) = self.sessions.remove(session_id) {
            // 同时移除 Agent 索引
            self.agent_index.remove(&session.agent_a);
            self.agent_index.remove(&session.agent_b);
            info!("移除会话: {} (session_id: {})", session_id, session_id);
            Some(session)
        } else {
            None
        }
    }

    /// 检查 Agent 是否正在对话中
    pub fn is_agent_in_dialogue(&self, agent_id: Uuid) -> bool {
        self.agent_index.contains_key(&agent_id)
    }

    /// 清理超时会话
    ///
    /// timeout: 会话超时时间（从最后活动开始计算）
    /// 返回被清理的会话数量
    #[allow(dead_code)]
    pub fn cleanup_timeout_sessions(&mut self, timeout: Duration) -> usize {
        // 收集需要清理的会话 ID
        let sessions_to_remove: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.is_timeout(timeout))
            .map(|(id, _)| id.clone())
            .collect();

        let removed_count = sessions_to_remove.len();

        // 移除超时会话
        for session_id in &sessions_to_remove {
            if let Some(session) = self.remove_session(session_id) {
                warn!(
                    "会话超时已清理: {} (agents: {} <-> {}, 持续: {:?})",
                    session_id,
                    session.agent_a,
                    session.agent_b,
                    session.created_at.elapsed()
                );
            }
        }

        if removed_count > 0 {
            info!("清理了 {} 个超时对话会话", removed_count);
        }

        removed_count
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dialogue_session_creation() {
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();
        let session = DialogueSession::new(agent_a, agent_b);

        assert_eq!(session.agent_a, agent_a);
        assert_eq!(session.agent_b, agent_b);
        assert_eq!(session.message_count, 0);
        assert_eq!(session.status, SessionStatus::Pending);
        assert!(!session.session_id.is_empty());
    }

    #[test]
    fn test_involves() {
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();
        let agent_c = Uuid::new_v4();
        let session = DialogueSession::new(agent_a, agent_b);

        assert!(session.involves(agent_a));
        assert!(session.involves(agent_b));
        assert!(!session.involves(agent_c));
    }

    #[test]
    fn test_get_partner() {
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();
        let session = DialogueSession::new(agent_a, agent_b);

        assert_eq!(session.get_partner(agent_a), Some(agent_b));
        assert_eq!(session.get_partner(agent_b), Some(agent_a));
    }

    #[test]
    fn test_increment_message_count() {
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();
        let mut session = DialogueSession::new(agent_a, agent_b);

        assert_eq!(session.message_count, 0);
        session.increment_message_count();
        assert_eq!(session.message_count, 1);
        session.increment_message_count();
        assert_eq!(session.message_count, 2);
    }

    #[test]
    fn test_message_limit() {
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();
        let mut session = DialogueSession::new(agent_a, agent_b);

        // max_messages=10 means total limit is 20
        assert!(!session.is_message_limit_reached(10));

        // Add 19 messages - should not reach limit yet
        for _ in 0..19 {
            session.increment_message_count();
        }
        assert!(!session.is_message_limit_reached(10));

        // Add 1 more - total 20, should reach limit
        session.increment_message_count();
        assert!(session.is_message_limit_reached(10));
    }

    #[test]
    fn test_session_registry() {
        let mut registry = SessionRegistry::new();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();

        // 创建会话
        let session = registry.create_session(agent_a, agent_b);
        assert_eq!(session.status, SessionStatus::Pending);

        // 检查 Agent 是否在对话中
        assert!(registry.is_agent_in_dialogue(agent_a));
        assert!(registry.is_agent_in_dialogue(agent_b));

        // 获取会话
        let retrieved = registry.get_session(&session.session_id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().session_id, session.session_id);

        // 获取 Agent 会话
        let agent_session = registry.get_agent_session(agent_a);
        assert!(agent_session.is_some());
        assert_eq!(agent_session.unwrap().session_id, session.session_id);
    }

    #[test]
    fn test_remove_session() {
        let mut registry = SessionRegistry::new();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();

        let session = registry.create_session(agent_a, agent_b);

        // 移除会话
        let removed = registry.remove_session(&session.session_id);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().session_id, session.session_id);

        // 检查索引已清理
        assert!(!registry.is_agent_in_dialogue(agent_a));
        assert!(!registry.is_agent_in_dialogue(agent_b));

        // 再次移除应返回 None
        let removed_again = registry.remove_session(&session.session_id);
        assert!(removed_again.is_none());
    }

    #[test]
    fn test_update_session() {
        let mut registry = SessionRegistry::new();
        let agent_a = Uuid::new_v4();
        let agent_b = Uuid::new_v4();

        let session = registry.create_session(agent_a, agent_b);
        assert_eq!(session.status, SessionStatus::Pending);

        // 更新会话状态
        let mut updated = session.clone();
        updated.status = SessionStatus::Active;

        let previous = registry.update_session(&session.session_id, updated);
        assert!(previous.is_some());

        // 验证更新后的状态
        let retrieved = registry.get_session(&session.session_id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().status, SessionStatus::Active);
    }
}
