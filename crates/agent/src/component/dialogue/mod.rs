use std::collections::HashMap;
use tracing::warn;
use uuid::Uuid;

pub const PENDING_SESSION_PREFIX: &str = "pending_";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogueRole {
    Own,
    Partner,
}

#[derive(Debug, Clone)]
struct DialogueMessageEntry {
    role: DialogueRole,
    content: String,
}

#[derive(Debug, Clone)]
pub struct DialogueSession {
    session_id: String,
    partner_id: Uuid,
    partner_name: String,
    messages: Vec<DialogueMessageEntry>,
    last_active_tick: i64,
    is_active: bool,
}

#[derive(Debug)]
pub struct DialogueContextManager {
    sessions: HashMap<String, DialogueSession>,
    partner_to_session: HashMap<Uuid, String>,
    max_sessions: usize,
    max_rounds_per_session: usize,
    session_timeout_ticks: i64,
    dialogue_action_types: Vec<String>,
}

impl DialogueContextManager {
    pub fn new(
        max_sessions: usize,
        max_rounds: usize,
        session_timeout_ticks: i64,
        dialogue_action_types: Vec<String>,
    ) -> Self {
        Self {
            sessions: HashMap::new(),
            partner_to_session: HashMap::new(),
            max_sessions,
            max_rounds_per_session: max_rounds,
            session_timeout_ticks,
            dialogue_action_types,
        }
    }

    pub fn is_dialogue_action(&self, action_type: &str) -> bool {
        self.dialogue_action_types.iter().any(|t| t == action_type)
    }

    pub fn register_session(&mut self, session_id: &str, partner_id: Uuid, tick_id: i64) {
        if let Some(old_session_id) = self.partner_to_session.remove(&partner_id) {
            #[allow(clippy::collapsible_if)]
            if old_session_id != session_id {
                self.sessions.remove(&old_session_id);
            }
        }
        self.sessions
            .entry(session_id.to_string())
            .or_insert_with(|| DialogueSession {
                session_id: session_id.to_string(),
                partner_id,
                partner_name: String::new(),
                messages: Vec::new(),
                last_active_tick: tick_id,
                is_active: true,
            });
        // IMPORTANT: always update partner_id even if session already exists
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.partner_id = partner_id;
        }
        self.partner_to_session
            .insert(partner_id, session_id.to_string());

        if self.sessions.len() > self.max_sessions {
            self.cleanup_inactive();
        }
    }

    pub fn migrate_session(
        &mut self,
        old_session_id: &str,
        new_session_id: &str,
        partner_id: Uuid,
        tick_id: i64,
    ) {
        if let Some(mut session) = self.sessions.remove(old_session_id) {
            session.session_id = new_session_id.to_string();
            self.sessions.insert(new_session_id.to_string(), session);
        } else {
            warn!(
                "migrate_session: {} not found, creating new session",
                old_session_id
            );
            self.register_session(new_session_id, partner_id, tick_id);
            return;
        }
        self.partner_to_session
            .insert(partner_id, new_session_id.to_string());
    }

    pub fn get_session_id_by_partner(&self, partner_id: &Uuid) -> Option<&str> {
        self.partner_to_session.get(partner_id).map(|s| s.as_str())
    }

    pub fn add_message(
        &mut self,
        session_id: &str,
        partner_id: Uuid,
        role: DialogueRole,
        content: &str,
        tick_id: i64,
    ) {
        let session = self
            .sessions
            .entry(session_id.to_string())
            .or_insert_with(|| DialogueSession {
                session_id: session_id.to_string(),
                partner_id,
                partner_name: String::new(),
                messages: Vec::new(),
                last_active_tick: tick_id,
                is_active: true,
            });

        session.partner_id = partner_id;
        session.last_active_tick = tick_id;
        session.is_active = true;

        if session.messages.len() >= self.max_rounds_per_session {
            session.messages.remove(0);
        }
        session.messages.push(DialogueMessageEntry {
            role,
            content: content.to_string(),
        });

        if self.sessions.len() > self.max_sessions {
            self.cleanup_inactive();
        }
    }

    pub fn close_session(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.is_active = false;
        }
    }

    pub fn end_session(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.remove(session_id) {
            self.partner_to_session.remove(&session.partner_id);
        }
    }

    fn cleanup_inactive(&mut self) {
        let inactive: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| !s.is_active)
            .map(|(k, _)| k.clone())
            .collect();

        for key in inactive {
            if let Some(session) = self.sessions.remove(&key) {
                self.partner_to_session.remove(&session.partner_id);
            }
        }

        while self.sessions.len() > self.max_sessions {
            let oldest_key = self
                .sessions
                .iter()
                .min_by_key(|(_, s)| s.last_active_tick)
                .map(|(k, _)| k.clone());

            if let Some(key) = oldest_key {
                if let Some(session) = self.sessions.remove(&key) {
                    if session.is_active {
                        warn!(
                            "cleanup_inactive: evicting active session {} (partner={}, last_tick={}) to make room",
                            session.session_id, session.partner_id, session.last_active_tick
                        );
                    }
                    self.partner_to_session.remove(&session.partner_id);
                }
            } else {
                break;
            }
        }
    }

    pub fn cleanup_timed_out(&mut self, current_tick: i64) {
        let timeout_threshold = current_tick - self.session_timeout_ticks;

        let timed_out: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.is_active && s.last_active_tick < timeout_threshold)
            .map(|(k, _)| k.clone())
            .collect();

        for key in timed_out {
            if let Some(session) = self.sessions.get_mut(&key) {
                session.is_active = false;
            }
        }
    }

    pub fn get_active_sessions_context(&self) -> String {
        let active_sessions: Vec<&DialogueSession> =
            self.sessions.values().filter(|s| s.is_active).collect();

        if active_sessions.is_empty() {
            return String::new();
        }

        let mut lines = Vec::new();
        for session in active_sessions {
            let partner_display = if session.partner_name.is_empty() {
                format!("角色({})", session.partner_id)
            } else {
                session.partner_name.clone()
            };

            lines.push(format!(
                "## 与{}的对话 (session: {})\n",
                partner_display, session.session_id
            ));

            for msg in &session.messages {
                let role_str = match msg.role {
                    DialogueRole::Own => "你",
                    DialogueRole::Partner => &partner_display,
                };
                // 注意：注入源保持原文（不影响 agent 推理），脱敏在 trace record 时做
                lines.push(format!("- {}: {}", role_str, msg.content));
            }
            lines.push(String::new());
        }

        lines.join("\n")
    }

    pub fn get_session_history(&self, session_id: &str) -> Option<&DialogueSession> {
        self.sessions.get(session_id)
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn active_session_count(&self) -> usize {
        self.sessions.values().filter(|s| s.is_active).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_manager() -> DialogueContextManager {
        DialogueContextManager::new(3, 5, 10, vec!["说话".to_string()])
    }

    #[test]
    fn test_is_dialogue_action() {
        let manager = create_test_manager();
        assert!(manager.is_dialogue_action("说话"));
        assert!(!manager.is_dialogue_action("休整"));
    }

    #[test]
    fn test_add_message_creates_session() {
        let mut manager = create_test_manager();
        let partner_id = Uuid::new_v4();

        manager.add_message("session1", partner_id, DialogueRole::Own, "你好", 1);

        assert_eq!(manager.session_count(), 1);
        let history = manager.get_session_history("session1").unwrap();
        assert_eq!(history.messages.len(), 1);
        assert_eq!(history.messages[0].content, "你好");
    }

    #[test]
    fn test_add_message_appends_to_existing_session() {
        let mut manager = create_test_manager();
        let partner_id = Uuid::new_v4();

        manager.add_message("session1", partner_id, DialogueRole::Own, "你好", 1);
        manager.add_message(
            "session1",
            partner_id,
            DialogueRole::Partner,
            "你好，我是...",
            2,
        );

        let history = manager.get_session_history("session1").unwrap();
        assert_eq!(history.messages.len(), 2);
    }

    #[test]
    fn test_max_rounds_eviction() {
        let mut manager = DialogueContextManager::new(10, 3, 100, vec![]);
        let partner_id = Uuid::new_v4();

        for i in 1..=5 {
            manager.add_message(
                "session1",
                partner_id,
                DialogueRole::Own,
                &format!("消息{}", i),
                i,
            );
        }

        let history = manager.get_session_history("session1").unwrap();
        assert_eq!(history.messages.len(), 3);
        assert_eq!(history.messages[0].content, "消息3");
        assert_eq!(history.messages[1].content, "消息4");
        assert_eq!(history.messages[2].content, "消息5");
    }

    #[test]
    fn test_register_session_clears_old_mapping() {
        let mut manager = create_test_manager();
        let partner_id = Uuid::new_v4();

        manager.register_session("old_session", partner_id, 1);
        assert_eq!(
            manager.get_session_id_by_partner(&partner_id),
            Some("old_session")
        );

        manager.register_session("new_session", partner_id, 2);
        assert_eq!(
            manager.get_session_id_by_partner(&partner_id),
            Some("new_session")
        );
        assert!(manager.get_session_history("old_session").is_none());
        assert!(manager.get_session_history("new_session").is_some());
    }

    #[test]
    fn test_migrate_session() {
        let mut manager = create_test_manager();
        let partner_id = Uuid::new_v4();

        manager.add_message("pending_xyz", partner_id, DialogueRole::Own, "你好", 1);
        manager.register_session("pending_xyz", partner_id, 1);

        manager.migrate_session("pending_xyz", "real_session_123", partner_id, 2);

        assert!(manager.get_session_history("pending_xyz").is_none());
        let history = manager.get_session_history("real_session_123").unwrap();
        assert_eq!(history.messages.len(), 1);
        assert_eq!(
            manager.get_session_id_by_partner(&partner_id),
            Some("real_session_123")
        );
    }

    #[test]
    fn test_migrate_session_fallback_on_missing() {
        let mut manager = create_test_manager();
        let partner_id = Uuid::new_v4();

        manager.migrate_session("nonexistent", "new_session", partner_id, 1);

        assert!(manager.get_session_history("new_session").is_some());
        assert_eq!(
            manager.get_session_id_by_partner(&partner_id),
            Some("new_session")
        );
    }

    #[test]
    fn test_close_session() {
        let mut manager = create_test_manager();
        let partner_id = Uuid::new_v4();

        manager.add_message("session1", partner_id, DialogueRole::Own, "你好", 1);
        manager.close_session("session1");

        let history = manager.get_session_history("session1").unwrap();
        assert!(!history.is_active);
    }

    #[test]
    fn test_end_session_removes_from_index() {
        let mut manager = create_test_manager();
        let partner_id = Uuid::new_v4();

        manager.register_session("session1", partner_id, 1);
        assert_eq!(
            manager.get_session_id_by_partner(&partner_id),
            Some("session1")
        );

        manager.end_session("session1");

        assert!(manager.get_session_history("session1").is_none());
        assert!(manager.get_session_id_by_partner(&partner_id).is_none());
    }

    #[test]
    fn test_cleanup_timed_out() {
        let mut manager = DialogueContextManager::new(10, 5, 5, vec![]);
        let partner_id = Uuid::new_v4();

        manager.add_message("session1", partner_id, DialogueRole::Own, "消息1", 1);
        manager.add_message("session2", partner_id, DialogueRole::Own, "消息2", 10);

        manager.cleanup_timed_out(15);

        let history1 = manager.get_session_history("session1").unwrap();
        assert!(!history1.is_active);

        let history2 = manager.get_session_history("session2").unwrap();
        assert!(history2.is_active);
    }

    #[test]
    fn test_get_active_sessions_context() {
        let mut manager = create_test_manager();
        let partner_id = Uuid::new_v4();

        manager.add_message("session1", partner_id, DialogueRole::Own, "你好", 1);
        manager.add_message(
            "session1",
            partner_id,
            DialogueRole::Partner,
            "你好，我是...",
            2,
        );

        let context = manager.get_active_sessions_context();
        assert!(context.contains("与"));
        assert!(context.contains("你好"));
        assert!(context.contains("session1"));
    }

    #[test]
    fn test_max_sessions_eviction() {
        let mut manager = DialogueContextManager::new(2, 10, 100, vec![]);
        let partner1 = Uuid::new_v4();
        let partner2 = Uuid::new_v4();
        let partner3 = Uuid::new_v4();

        manager.add_message("s1", partner1, DialogueRole::Own, "s1", 1);
        manager.add_message("s2", partner2, DialogueRole::Own, "s2", 2);
        manager.close_session("s1");
        manager.add_message("s3", partner3, DialogueRole::Own, "s3", 3);

        assert!(manager.get_session_history("s1").is_none());
        assert!(manager.get_session_history("s2").is_some());
        assert!(manager.get_session_history("s3").is_some());
        assert!(manager.get_session_id_by_partner(&partner1).is_none());
    }

    #[test]
    fn test_cleanup_inactive_clears_partner_index() {
        let mut manager = DialogueContextManager::new(2, 10, 100, vec![]);
        let partner1 = Uuid::new_v4();
        let partner2 = Uuid::new_v4();
        let partner3 = Uuid::new_v4();

        manager.register_session("s1", partner1, 1);
        manager.register_session("s2", partner2, 2);
        manager.close_session("s1");
        manager.register_session("s3", partner3, 3);

        assert!(manager.get_session_id_by_partner(&partner1).is_none());
        assert_eq!(manager.get_session_id_by_partner(&partner2), Some("s2"));
        assert_eq!(manager.get_session_id_by_partner(&partner3), Some("s3"));
    }
}
