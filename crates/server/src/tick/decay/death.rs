// ============================================================================
// 死亡通知（自 decay.rs 拆分，原文件超 800 行上限）
// ============================================================================
//
// DeathNotification 构建、目击者筛选与目击事件；生产逻辑与测试同迁。
// ============================================================================

use uuid::Uuid;

use crate::models::{AgentState, WorldEvent, WorldEventType};

/// 死亡通知（用于立即推送）
///
/// 当Agent死亡时创建，包含死亡相关的完整信息
/// 用于通过WebSocket立即推送给Agent
#[derive(Debug, Clone)]
pub struct DeathNotification {
    /// 死亡Agent的ID
    pub agent_id: Uuid,
    /// 死亡原因代码（如 "satiation", "hydration", "hp"）
    pub cause: String,
    /// 死亡描述信息
    pub description: String,
    /// 死亡地点
    pub location: String,
    /// 死亡发生的Tick ID
    pub tick_id: i64,
    /// 死亡时间戳（毫秒）
    pub died_at: i64,
}

impl DeathNotification {
    /// 创建新的死亡通知
    pub fn new(
        agent_id: Uuid,
        cause: String,
        description: String,
        location: String,
        tick_id: i64,
    ) -> Self {
        Self {
            agent_id,
            cause,
            description,
            location,
            tick_id,
            died_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// 生成目击者视角的死亡事件描述（具名化）。
    ///
    /// 涌现行为（哀悼/记仇/避讳）依赖目击者记住"谁"死了，而非"有人"死了。
    /// 姓名缺失时回退到匿名描述（DashMap 已被清理等边界情况）。
    pub fn witness_description(&self, deceased_name: Option<&str>) -> String {
        match deceased_name {
            Some(name) if !name.is_empty() => {
                format!("{}在 {} 亡故：{}", name, self.location, self.description)
            }
            _ => format!("有人在 {} 亡故：{}", self.location, self.description),
        }
    }
}

/// 目击者筛选：同节点 + 存活 + 排除死者。
///
/// 与 broadcast_speak_to_location / send_reactive_world_state 的筛选条件对称；
/// is_alive 是对 DashMap 中可能残留的死亡状态的防御性过滤。
pub fn select_witnesses(states: &[AgentState], location: &str, deceased_id: Uuid) -> Vec<Uuid> {
    states
        .iter()
        .filter(|s| s.node_id == location && s.is_alive && s.agent_id != deceased_id)
        .map(|s| s.agent_id)
        .collect()
}

/// 构建目击者死亡事件（具名）。
///
/// 涌现行为（哀悼/记仇/避讳）依赖目击者记住"谁"死了，metadata.agent_id
/// 同时是 Agent 端 find_self_death 区分"目击"与"自身死亡"的唯一依据。
pub fn build_witness_death_event(
    notif: &DeathNotification,
    deceased_name: Option<&str>,
) -> WorldEvent {
    WorldEvent {
        event_type: WorldEventType::DeathNotification,
        tick_id: notif.tick_id,
        description: notif.witness_description(deceased_name),
        metadata: serde_json::json!({
            "agent_id": notif.agent_id.to_string(),
            "agent_name": deceased_name,
            "cause": notif.cause,
            "location": notif.location,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::super::apply_decay_and_environmental_damage;
    use super::*;

    fn make_notification() -> DeathNotification {
        DeathNotification::new(
            Uuid::new_v4(),
            "satiation".to_string(),
            "饥渴交加，体力不支".to_string(),
            "龙门大堂".to_string(),
            42,
        )
    }

    // ---- select_witnesses ----

    fn make_agent(id: Uuid, node: &str, alive: bool) -> AgentState {
        crate::game_data::init_test_registry();
        let mut s = AgentState::new(id, 1);
        s.node_id = node.to_string();
        s.is_alive = alive;
        s
    }

    #[test]
    fn select_witnesses_keeps_same_node_alive_and_excludes_deceased() {
        let deceased = Uuid::new_v4();
        let witness_a = Uuid::new_v4();
        let witness_b = Uuid::new_v4();
        let other_node = Uuid::new_v4();
        let dead_linger = Uuid::new_v4();
        let states = vec![
            make_agent(deceased, "龙门大堂", true),
            make_agent(witness_a, "龙门大堂", true),
            make_agent(witness_b, "龙门大堂", true),
            make_agent(other_node, "后山小径", true),
            make_agent(dead_linger, "龙门大堂", false),
        ];

        let witnesses = select_witnesses(&states, "龙门大堂", deceased);

        assert_eq!(witnesses.len(), 2, "只有同节点存活者（排除死者与死亡残留）");
        assert!(witnesses.contains(&witness_a));
        assert!(witnesses.contains(&witness_b));
        assert!(!witnesses.contains(&deceased), "死者自身不得成为目击者");
        assert!(
            !witnesses.contains(&dead_linger),
            "死亡状态残留者不得成为目击者"
        );
    }

    #[test]
    fn select_witnesses_excludes_deceased_even_with_stale_alive_flag() {
        // 防御性场景：死者已被移出 DashMap 的时序被破坏，快照里仍是 alive=true
        let deceased = Uuid::new_v4();
        let states = vec![make_agent(deceased, "龙门大堂", true)];

        let witnesses = select_witnesses(&states, "龙门大堂", deceased);

        assert!(witnesses.is_empty(), "显式排除死者优先于 is_alive 标记");
    }

    #[test]
    fn select_witnesses_empty_when_no_states() {
        assert!(select_witnesses(&[], "龙门大堂", Uuid::new_v4()).is_empty());
    }

    // ---- build_witness_death_event ----

    #[test]
    fn witness_event_metadata_is_complete() {
        let deceased = Uuid::new_v4();
        let notif = DeathNotification::new(
            deceased,
            "combat".to_string(),
            "在战斗中被杀害".to_string(),
            "龙门大堂".to_string(),
            42,
        );

        let event = build_witness_death_event(&notif, Some("李四"));

        assert_eq!(event.event_type, WorldEventType::DeathNotification);
        assert_eq!(event.tick_id, 42);
        // agent_id 是 Agent 端 find_self_death 区分目击/自死的唯一依据
        assert_eq!(
            event.metadata.get("agent_id").and_then(|v| v.as_str()),
            Some(deceased.to_string().as_str())
        );
        assert_eq!(
            event.metadata.get("agent_name").and_then(|v| v.as_str()),
            Some("李四")
        );
        assert_eq!(
            event.metadata.get("cause").and_then(|v| v.as_str()),
            Some("combat")
        );
        assert_eq!(
            event.metadata.get("location").and_then(|v| v.as_str()),
            Some("龙门大堂")
        );
        assert!(event.description.contains("李四"));
    }

    #[test]
    fn witness_event_anonymous_name_serializes_as_null() {
        let notif = make_notification();
        let event = build_witness_death_event(&notif, None);

        assert!(event.description.starts_with("有人"));
        assert!(event.metadata.get("agent_name").unwrap().is_null());
    }

    #[test]
    fn witness_description_includes_name_when_known() {
        let n = make_notification();
        let desc = n.witness_description(Some("张三"));
        assert_eq!(desc, "张三在 龙门大堂 亡故：饥渴交加，体力不支");
        assert!(desc.contains("张三"), "描述必须包含死者姓名");
        assert!(!desc.contains("有人"), "已知姓名时不应使用匿名描述");
    }

    #[test]
    fn witness_description_falls_back_to_anonymous() {
        let n = make_notification();
        let desc = n.witness_description(None);
        assert_eq!(desc, "有人在 龙门大堂 亡故：饥渴交加，体力不支");
    }

    #[test]
    fn witness_description_falls_back_when_name_empty() {
        let n = make_notification();
        // 空姓名等价于未知（DB JOIN 缺失等边界）
        let desc = n.witness_description(Some(""));
        assert!(desc.starts_with("有人"), "空姓名应回退匿名描述");
    }
    // ============================================================================
    // 死亡通知集成测试
    // ============================================================================

    /// 测试饥饿死亡时创建死亡通知
    #[test]
    fn test_satiation_death_creates_notification() {
        crate::game_data::init_test_registry();

        // 创建一个饱食度极低的 Agent
        let mut agent = AgentState::new(Uuid::new_v4(), 1);
        agent.is_alive = true;
        agent.node_id = "test_location".to_string();

        // 设置饱食度为 0（触发死亡条件）
        // 根据配置，satiation 的 death_condition 是 equals 0
        agent.status.set("satiation", 0).unwrap();

        let tick_id = 100;
        let agents = vec![agent];

        // 执行衰减
        let (updated_agents, dead_agents, events, death_notifications) =
            apply_decay_and_environmental_damage(
                tick_id,
                agents,
                &std::collections::HashSet::new(),
            );

        // 验证死亡通知
        assert_eq!(death_notifications.len(), 1, "应该创建一个死亡通知");

        let notification = &death_notifications[0];
        assert_eq!(notification.cause, "satiation", "死亡原因应该是 satiation");
        assert!(
            notification.description.contains("饥饿"),
            "描述应该包含饥饿相关文字，实际描述: {}",
            notification.description
        );
        assert_eq!(notification.location, "test_location");
        assert_eq!(notification.tick_id, tick_id);

        // 验证 agent 已标记为死亡
        assert_eq!(dead_agents.len(), 1);
        assert!(!updated_agents[0].is_alive);

        // 验证创建了死亡事件
        assert_eq!(events.len(), 1);
        let (event_agent_id, event) = &events[0];
        assert_eq!(*event_agent_id, updated_agents[0].agent_id);
        assert_eq!(event.event_type, WorldEventType::DeathNotification);
    }

    /// 测试口渴死亡时创建死亡通知
    #[test]
    fn test_hydration_death_creates_notification() {
        crate::game_data::init_test_registry();

        let mut agent = AgentState::new(Uuid::new_v4(), 1);
        agent.is_alive = true;
        agent.node_id = "test_location".to_string();

        // 设置饱饮度为 0（触发死亡条件）
        agent.status.set("hydration", 0).unwrap();

        let tick_id = 200;
        let agents = vec![agent];

        let (updated_agents, dead_agents, events, death_notifications) =
            apply_decay_and_environmental_damage(
                tick_id,
                agents,
                &std::collections::HashSet::new(),
            );

        // 验证死亡通知
        assert_eq!(death_notifications.len(), 1);
        let notification = &death_notifications[0];
        assert_eq!(notification.cause, "hydration");
        assert!(
            notification.description.contains("脱水"),
            "描述应该包含脱水相关文字，实际描述: {}",
            notification.description
        );
        assert_eq!(notification.location, "test_location");
        assert_eq!(notification.tick_id, tick_id);

        // 验证 agent 已标记为死亡
        assert_eq!(dead_agents.len(), 1);
        assert!(!updated_agents[0].is_alive);

        // 验证创建了死亡事件
        assert_eq!(events.len(), 1);
    }

    /// 测试存活 Agent 不产生死亡通知
    #[test]
    fn test_alive_agent_no_notification() {
        crate::game_data::init_test_registry();

        let mut agent = AgentState::new(Uuid::new_v4(), 1);
        agent.is_alive = true;
        agent.node_id = "test_location".to_string();

        // 设置健康值（高于死亡阈值）
        agent.status.set("satiation", 50).unwrap();
        agent.status.set("hydration", 50).unwrap();

        let agents = vec![agent];

        let (_, _, _, death_notifications) =
            apply_decay_and_environmental_damage(1, agents, &std::collections::HashSet::new());

        assert!(
            death_notifications.is_empty(),
            "存活 Agent 不应产生死亡通知"
        );
    }

    /// 测试多个 Agent 同时死亡时创建多个死亡通知
    #[test]
    fn test_multiple_deaths_create_multiple_notifications() {
        crate::game_data::init_test_registry();

        // 创建两个饱食度极低的 Agent
        let mut agent1 = AgentState::new(Uuid::new_v4(), 1);
        agent1.is_alive = true;
        agent1.node_id = "location_a".to_string();
        agent1.status.set("satiation", 0).unwrap();

        let mut agent2 = AgentState::new(Uuid::new_v4(), 1);
        agent2.is_alive = true;
        agent2.node_id = "location_b".to_string();
        agent2.status.set("hydration", 0).unwrap();

        let tick_id = 300;
        let agents = vec![agent1, agent2];

        let (updated_agents, dead_agents, events, death_notifications) =
            apply_decay_and_environmental_damage(
                tick_id,
                agents,
                &std::collections::HashSet::new(),
            );

        // 验证死亡通知
        assert_eq!(death_notifications.len(), 2, "应该创建两个死亡通知");
        assert_eq!(dead_agents.len(), 2, "应该有两个死亡 Agent");
        assert_eq!(events.len(), 2, "应该创建两个死亡事件");

        // 验证所有 agent 都已死亡
        for agent in &updated_agents {
            assert!(!agent.is_alive, "Agent {} 应该已死亡", agent.agent_id);
        }

        // 验证死亡原因
        let causes: Vec<&str> = death_notifications
            .iter()
            .map(|n| n.cause.as_str())
            .collect();
        assert!(causes.contains(&"satiation"), "应该包含饥饿死亡");
        assert!(causes.contains(&"hydration"), "应该包含口渴死亡");
    }

    /// 测试已死亡的 Agent 不会再次触发死亡通知
    #[test]
    fn test_already_dead_agent_no_notification() {
        crate::game_data::init_test_registry();

        // 创建一个已经死亡的 Agent
        let mut agent = AgentState::new(Uuid::new_v4(), 1);
        agent.is_alive = false; // 已死亡
        agent.node_id = "test_location".to_string();
        agent.status.set("satiation", 0).unwrap();

        let agents = vec![agent];

        let (_, _, _, death_notifications) =
            apply_decay_and_environmental_damage(1, agents, &std::collections::HashSet::new());

        // 已死亡的 Agent 不应再次产生死亡通知
        assert!(
            death_notifications.is_empty(),
            "已死亡的 Agent 不应再次产生死亡通知"
        );
    }
}
