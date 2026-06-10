use cyber_jianghu_protocol::WorldEvent;
use cyber_jianghu_protocol::WorldEventType;
use std::collections::HashMap;

/// 从 WorldEvent.metadata 提取 outcome 字符串
pub fn extract_outcome(
    event: &WorldEvent,
    outcome_mapping: &HashMap<String, String>,
) -> Option<String> {
    match event.event_type {
        WorldEventType::ActionResult => event
            .metadata
            .get("success")
            .and_then(|v| v.as_bool())
            .map(|s| if s { "success" } else { "failure" }.to_string()),
        WorldEventType::SocialInteraction => event
            .metadata
            .get("action")
            .and_then(|v| v.as_str())
            .and_then(|action| outcome_mapping.get(action).cloned()),
        _ => None,
    }
}

/// 获取事件的 category 字符串（WorldEventType → Display）
pub fn event_category(event: &WorldEvent) -> String {
    event.event_type.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_action_event(success: bool) -> WorldEvent {
        WorldEvent {
            event_type: WorldEventType::ActionResult,
            tick_id: 1,
            description: "test".into(),
            metadata: json!({"success": success}),
        }
    }

    fn make_social_event(action: &str) -> WorldEvent {
        WorldEvent {
            event_type: WorldEventType::SocialInteraction,
            tick_id: 1,
            description: "test".into(),
            metadata: json!({"action": action}),
        }
    }

    #[test]
    fn test_action_result_success() {
        let event = make_action_event(true);
        assert_eq!(
            extract_outcome(&event, &HashMap::new()),
            Some("success".into())
        );
    }

    #[test]
    fn test_action_result_failure() {
        let event = make_action_event(false);
        assert_eq!(
            extract_outcome(&event, &HashMap::new()),
            Some("failure".into())
        );
    }

    #[test]
    fn test_social_interaction_with_mapping() {
        let event = make_social_event("予");
        let mut mapping = HashMap::new();
        mapping.insert("予".into(), "friendly".into());
        assert_eq!(extract_outcome(&event, &mapping), Some("friendly".into()));
    }

    #[test]
    fn test_social_interaction_no_mapping_returns_none() {
        let event = make_social_event("unknown_action");
        assert_eq!(
            extract_outcome(&event, &HashMap::new()),
            None,
            "未映射的社交动作应返回 None"
        );
    }

    #[test]
    fn test_unsupported_event_type() {
        let event = WorldEvent {
            event_type: WorldEventType::TimeUpdate,
            tick_id: 1,
            description: "test".into(),
            metadata: json!({}),
        };
        assert_eq!(extract_outcome(&event, &HashMap::new()), None);
    }

    #[test]
    fn test_event_category_string() {
        let event = make_action_event(true);
        assert_eq!(event_category(&event), "action_result");
    }
}
