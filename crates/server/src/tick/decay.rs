// ============================================================================
// OpenClaw Cyber-Jianghu MVP - Decay Module
// ============================================================================
//
// 本模块负责处理Agent的生理值衰减和环境伤害
//
// 功能：
// - 应用基础生理值衰减（饥饿、口渴、体力）
// - 应用环境压力伤害
// - 处理死亡Agent的检测和清理
// - 生成死亡通知用于立即推送
// ============================================================================

use tracing::{debug, warn};
use uuid::Uuid;

use crate::models::{AgentState, WorldEventType};

use crate::game_data::registry_or_panic;
use cyber_jianghu_protocol::DeathInfo;

/// 死亡通知（用于立即推送）
///
/// 当Agent死亡时创建，包含死亡相关的完整信息
/// 用于通过WebSocket立即推送给Agent
#[derive(Debug, Clone)]
pub struct DeathNotification {
    /// 死亡Agent的ID
    pub agent_id: Uuid,
    /// 死亡原因代码（如 "hunger", "thirst", "hp"）
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
}

/// 应用生理值衰减和环境压力伤害
///
/// 生理值衰减逻辑由 StatusComponent 统一处理（基于配置），包括：
/// - 饥饿值、口渴值、体力等属性的自然变化
///
/// 环境压力伤害（如果启用）：
/// - 基于当前位置的 environmental_damage 配置
/// - 如果 > 0，则扣除相应 HP
///
/// 衰减处理结果
#[allow(clippy::type_complexity)]
pub type DecayResult = (
    Vec<AgentState>,
    Vec<Uuid>,
    Vec<(Uuid, crate::models::WorldEvent)>,
    Vec<DeathNotification>,
);

/// 返回值：(更新后的Agent状态, 本Tick死亡的Agent ID列表, 事件列表, 死亡通知列表)
pub fn apply_decay_and_environmental_damage(
    tick_id: i64,
    mut agent_states: Vec<AgentState>,
) -> DecayResult {
    let mut dead_agents = Vec::new();
    let mut events = Vec::new();
    let mut death_notifications = Vec::new();

    // 获取位置注册表
    let registry = registry_or_panic();
    let location_registry = registry.location_registry.read().unwrap();

    for state in &mut agent_states {
        let was_alive = state.is_alive;
        let agent_id = state.agent_id;
        let location = state.node_id.clone();

        // 应用基础生理值衰减
        // 传递 tick_id，以便 apply_decay 可以获取季节信息
        // 返回触发死亡的属性名（如果有）
        let death_attr_name = state.apply_decay(tick_id);

        // 如果Agent因衰减死亡，创建死亡通知
        if let Some(attr_name) = death_attr_name {
            if was_alive {
                dead_agents.push(agent_id);

                // 获取死亡信息
                let death_info = registry.get_death_info(&attr_name);

                let (cause, description) = match death_info {
                    Some(DeathInfo { cause, message }) => (cause, message),
                    None => {
                        // 使用配置的默认值，而非硬编码
                        let defaults = registry.get_unknown_death_info();
                        (defaults.cause, defaults.message)
                    }
                };

                warn!("Agent {} 已死亡（{}），将清空背包", agent_id, cause);

                // 创建死亡事件
                let death_event = crate::models::WorldEvent {
                    event_type: WorldEventType::DeathNotification,
                    tick_id,
                    description: description.clone(),
                    metadata: serde_json::json!({
                        "cause": &cause,
                        "location": &location,
                    }),
                };
                events.push((agent_id, death_event));

                // 创建死亡通知
                let notification =
                    DeathNotification::new(agent_id, cause, description, location, tick_id);
                death_notifications.push(notification);
            }
            continue; // 已死亡，跳过环境伤害检查
        }

        // 应用环境压力伤害
        // 只有存活时才应用
        if state.is_alive {
            // 获取当前位置的环境伤害值
            // 优先使用节点配置的值，如果没有配置则默认为 0（无伤害）
            let damage = location_registry
                .get_node(&state.node_id)
                .and_then(|node| node.environmental_damage)
                .unwrap_or(0);

            if damage > 0 {
                state.take_damage(damage);
                debug!(
                    "Agent {} 在 {} 受到环境伤害 -{} HP",
                    agent_id, state.node_id, damage
                );

                // 记录环境伤害事件
                let event = crate::models::WorldEvent {
                    event_type: WorldEventType::EnvironmentalChange,
                    tick_id,
                    description: format!("你在 {} 受到环境伤害，HP 减少 {}", state.node_id, damage),
                    metadata: serde_json::json!({
                        "cause": "environmental_damage",
                        "location": state.node_id.clone(),
                        "damage": damage,
                    }),
                };
                events.push((agent_id, event));

                // 检查环境伤害是否导致死亡
                if was_alive && !state.is_alive {
                    dead_agents.push(agent_id);

                    // 环境伤害死亡使用 hp 作为原因
                    let death_info = registry.get_death_info("hp");

                    let (cause, description) = match death_info {
                        Some(DeathInfo { cause, message }) => (cause, message),
                        None => {
                            // 使用配置的环境伤害默认值，而非硬编码
                            let defaults = registry.get_environmental_death_info();
                            (defaults.cause, defaults.message)
                        }
                    };

                    warn!("Agent {} 已死亡（{}），将清空背包", agent_id, cause);

                    // 创建死亡事件
                    let death_event = crate::models::WorldEvent {
                        event_type: WorldEventType::DeathNotification,
                        tick_id,
                        description: description.clone(),
                        metadata: serde_json::json!({
                            "cause": &cause,
                            "location": &state.node_id,
                        }),
                    };
                    events.push((agent_id, death_event));

                    // 创建死亡通知
                    let notification = DeathNotification::new(
                        agent_id,
                        cause,
                        description,
                        state.node_id.clone(),
                        tick_id,
                    );
                    death_notifications.push(notification);
                }
            }
        } else if !was_alive {
            // 已经死亡的Agent（在本次tick开始前就已死亡）
            debug!("Agent {} 已经死亡", agent_id);
        }
    }

    // 处理物品耐久度自然衰减
    // 异步操作需要 db_pool，这里先收集需要处理的物品 ID
    // TODO: 实现物品自然损坏逻辑 (Phase 2)
    // 逻辑：
    // 1. 获取所有 Agent 的背包
    // 2. 对每个物品，如果有 decay_rate > 0，则减少 durability
    // 3. 如果 durability <= 0，则移除物品
    // 4. 发送物品损坏通知

    (agent_states, dead_agents, events, death_notifications)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_decay_logic() {
        crate::game_data::init_test_registry();

        // 测试 AgentState 的衰减逻辑
        // 根据 PRD，白板重生初始值：HP=100, 体力=100, 饥饿=50, 口渴=50
        let mut state = AgentState::new(uuid::Uuid::new_v4(), 1);
        assert_eq!(state.status.get("hunger").unwrap_or(0), 50); // 初始饥饿值
        assert_eq!(state.status.get("thirst").unwrap_or(0), 50); // 初始口渴值
        assert_eq!(state.status.get("stamina").unwrap_or(0), 100); // 初始体力值

        // 应用衰减（饥饿 -5, 口渴 -5, 体力 +5）
        let _ = state.apply_decay(1);
        assert_eq!(state.status.get("hunger").unwrap_or(0), 45); // 50 - 5
        assert_eq!(state.status.get("thirst").unwrap_or(0), 45); // 50 - 5
        assert_eq!(state.status.get("stamina").unwrap_or(0), 100); // 已经是最大值，保持 100
    }

    // ============================================================================
    // 死亡通知集成测试
    // ============================================================================

    /// 测试饥饿死亡时创建死亡通知
    #[test]
    fn test_hunger_death_creates_notification() {
        crate::game_data::init_test_registry();

        // 创建一个饥饿值极低的 Agent
        let mut agent = AgentState::new(Uuid::new_v4(), 1);
        agent.is_alive = true;
        agent.node_id = "test_location".to_string();

        // 设置饥饿值为 0（触发死亡条件）
        // 根据配置，hunger 的 death_condition 是 equals 0
        agent.status.set("hunger", 0).unwrap();

        let tick_id = 100;
        let agents = vec![agent];

        // 执行衰减
        let (updated_agents, dead_agents, events, death_notifications) =
            apply_decay_and_environmental_damage(tick_id, agents);

        // 验证死亡通知
        assert_eq!(death_notifications.len(), 1, "应该创建一个死亡通知");

        let notification = &death_notifications[0];
        assert_eq!(notification.cause, "hunger", "死亡原因应该是 hunger");
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
    fn test_thirst_death_creates_notification() {
        crate::game_data::init_test_registry();

        let mut agent = AgentState::new(Uuid::new_v4(), 1);
        agent.is_alive = true;
        agent.node_id = "test_location".to_string();

        // 设置口渴值为 0（触发死亡条件）
        agent.status.set("thirst", 0).unwrap();

        let tick_id = 200;
        let agents = vec![agent];

        let (updated_agents, dead_agents, events, death_notifications) =
            apply_decay_and_environmental_damage(tick_id, agents);

        // 验证死亡通知
        assert_eq!(death_notifications.len(), 1);
        let notification = &death_notifications[0];
        assert_eq!(notification.cause, "thirst");
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
        agent.status.set("hunger", 50).unwrap();
        agent.status.set("thirst", 50).unwrap();

        let agents = vec![agent];

        let (_, _, _, death_notifications) = apply_decay_and_environmental_damage(1, agents);

        assert!(
            death_notifications.is_empty(),
            "存活 Agent 不应产生死亡通知"
        );
    }

    /// 测试多个 Agent 同时死亡时创建多个死亡通知
    #[test]
    fn test_multiple_deaths_create_multiple_notifications() {
        crate::game_data::init_test_registry();

        // 创建两个饥饿值极低的 Agent
        let mut agent1 = AgentState::new(Uuid::new_v4(), 1);
        agent1.is_alive = true;
        agent1.node_id = "location_a".to_string();
        agent1.status.set("hunger", 0).unwrap();

        let mut agent2 = AgentState::new(Uuid::new_v4(), 1);
        agent2.is_alive = true;
        agent2.node_id = "location_b".to_string();
        agent2.status.set("thirst", 0).unwrap();

        let tick_id = 300;
        let agents = vec![agent1, agent2];

        let (updated_agents, dead_agents, events, death_notifications) =
            apply_decay_and_environmental_damage(tick_id, agents);

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
        assert!(causes.contains(&"hunger"), "应该包含饥饿死亡");
        assert!(causes.contains(&"thirst"), "应该包含口渴死亡");
    }

    /// 测试已死亡的 Agent 不会再次触发死亡通知
    #[test]
    fn test_already_dead_agent_no_notification() {
        crate::game_data::init_test_registry();

        // 创建一个已经死亡的 Agent
        let mut agent = AgentState::new(Uuid::new_v4(), 1);
        agent.is_alive = false; // 已死亡
        agent.node_id = "test_location".to_string();
        agent.status.set("hunger", 0).unwrap();

        let agents = vec![agent];

        let (_, _, _, death_notifications) = apply_decay_and_environmental_damage(1, agents);

        // 已死亡的 Agent 不应再次产生死亡通知
        assert!(
            death_notifications.is_empty(),
            "已死亡的 Agent 不应再次产生死亡通知"
        );
    }
}
