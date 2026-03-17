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
// ============================================================================

use tracing::{debug, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::models::AgentState;

use crate::game_data::registry_or_panic;

/// 应用生理值衰减和环境压力伤害
///
/// 生理值衰减逻辑由 StatusComponent 统一处理（基于配置），包括：
/// - 饥饿值、口渴值、体力等属性的自然变化
///
/// 环境压力伤害（如果启用）：
/// - 基于当前位置的 environmental_damage 配置
/// - 如果 > 0，则扣除相应 HP
///
/// 返回值：(更新后的Agent状态, 本Tick死亡的Agent ID列表, 事件列表)
pub fn apply_decay_and_environmental_damage(
    _config: &Config,
    tick_id: i64,
    mut agent_states: Vec<AgentState>,
) -> (
    Vec<AgentState>,
    Vec<Uuid>,
    Vec<(Uuid, crate::models::WorldEvent)>,
) {
    let mut dead_agents = Vec::new();
    let mut events = Vec::new();

    // 获取位置注册表
    let registry = registry_or_panic();
    let location_registry = registry.location_registry.read().unwrap();

    for state in &mut agent_states {
        let was_alive = state.is_alive;
        let agent_id = state.agent_id;

        // 应用基础生理值衰减
        // 传递 tick_id，以便 apply_decay 可以获取季节信息
        state.apply_decay(tick_id);

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
                    event_type: "environmental_damage".to_string(),
                    tick_id,
                    description: format!("你在 {} 受到环境伤害，HP 减少 {}", state.node_id, damage),
                    metadata: serde_json::json!({
                        "cause": "environmental_damage",
                        "location": state.node_id,
                        "damage": damage,
                    }),
                };
                events.push((agent_id, event));
            }
        }

        // 如果Agent刚刚死亡（从存活变为死亡），记录下来
        if was_alive && !state.is_alive {
            warn!(
                "Agent {} 已死亡（饥饿、口渴或环境伤害），将清空背包",
                agent_id
            );
            dead_agents.push(agent_id);

            // 记录死亡事件
            let death_event = crate::models::WorldEvent {
                event_type: "action_result".to_string(),
                tick_id,
                description: "你因饥饿、口渴或环境伤害而死亡".to_string(),
                metadata: serde_json::json!({
                    "cause": "death_by_natural_causes",
                    "location": state.node_id,
                }),
            };
            events.push((agent_id, death_event));
        } else if !state.is_alive {
            // 已经死亡的Agent
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

    (agent_states, dead_agents, events)
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
        state.apply_decay(1);
        assert_eq!(state.status.get("hunger").unwrap_or(0), 45); // 50 - 5
        assert_eq!(state.status.get("thirst").unwrap_or(0), 45); // 50 - 5
        assert_eq!(state.status.get("stamina").unwrap_or(0), 100); // 已经是最大值，保持 100
    }
}
