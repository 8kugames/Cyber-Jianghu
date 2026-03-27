// ============================================================================
// 数据模型
// ============================================================================
//
// 重导出 protocol crate 中的类型
// Intent 的构建方法已在 protocol::types::Intent 中实现，无需重复定义
// ============================================================================

// 重导出 protocol 中的所有核心类型
pub use cyber_jianghu_protocol::{
    ActionType,
    AgentSelfState,
    AvailableAction,
    ClientMessage,
    Entity,
    GameRules,
    InitialItem, // 游戏规则相关类型
    Intent,
    InventoryItem,
    Location,
    SceneItem,
    // 消息类型
    ServerMessage,
    WorldEvent,
    // 数据类型
    WorldState,
    WorldTime,
};

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intent_creation() {
        let agent_id = uuid::Uuid::new_v4();
        let intent = Intent::new(agent_id, 1, "idle", None);
        assert_eq!(intent.action_type.as_str(), "idle");
        assert_eq!(intent.tick_id, 1);

        let intent = Intent::new(
            agent_id,
            2,
            "speak",
            Some(serde_json::json!({"content": "大家好"})),
        );
        assert_eq!(intent.action_type.as_str(), "speak");
        assert!(intent.action_data.is_some());
    }

    #[test]
    fn test_intent_with_thought() {
        let agent_id = uuid::Uuid::new_v4();
        let intent =
            Intent::new(agent_id, 1, "idle", None).with_thought("我需要休息一下".to_string());
        assert_eq!(intent.thought_log, Some("我需要休息一下".to_string()));
    }
}
