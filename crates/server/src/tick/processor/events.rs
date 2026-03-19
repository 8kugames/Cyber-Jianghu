//! 事件生成器
//!
//! 负责生成和管理 WorldEvent。

use crate::models::WorldEvent;

/// 事件生成器
pub struct EventBuilder;

impl EventBuilder {
    /// 创建新的生成器
    pub fn new() -> Self {
        Self
    }

    /// 构建动作结果事件
    #[allow(dead_code)]
    pub fn build_action_event(
        &self,
        _agent_id: uuid::Uuid,
        tick_id: i64,
        action: &str,
        success: bool,
    ) -> WorldEvent {
        WorldEvent {
            event_type: "action_result".to_string(),
            tick_id,
            description: if success {
                format!("执行 {} 成功", action)
            } else {
                format!("执行 {} 失败", action)
            },
            metadata: serde_json::json!({
                "action": action,
                "success": success,
            }),
        }
    }

    /// 构建状态变更事件
    #[allow(dead_code)]
    pub fn build_state_change_event(
        &self,
        _agent_id: uuid::Uuid,
        tick_id: i64,
        change_type: &str,
        details: serde_json::Value,
    ) -> WorldEvent {
        WorldEvent {
            event_type: "state_change".to_string(),
            tick_id,
            description: format!("状态变更: {}", change_type),
            metadata: details,
        }
    }
}

impl Default for EventBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_build_action_event() {
        let builder = EventBuilder::new();
        let agent_id = Uuid::new_v4();

        let event = builder.build_action_event(agent_id, 1, "speak", true);

        assert_eq!(event.event_type, "action_result");
        assert!(event.description.contains("成功"));
    }

    #[test]
    fn test_build_state_change_event() {
        let builder = EventBuilder::new();
        let agent_id = Uuid::new_v4();

        let event = builder.build_state_change_event(
            agent_id,
            1,
            "hp",
            serde_json::json!({"old": 100, "new": 90}),
        );

        assert_eq!(event.event_type, "state_change");
    }
}
