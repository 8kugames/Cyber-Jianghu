// ============================================================================
// OpenClaw Cyber-Jianghu 衰减引擎
// ============================================================================
//
// 每 tick 属性衰减、环境伤害与寿终判定；死亡通知/年龄换算拆分至子模块
// （death.rs / age.rs），re-export 保持 crate::tick::decay::* 路径不变。
// ============================================================================

mod age;
mod death;

pub use age::{compute_age_years, compute_starting_age_ticks};
pub use death::{DeathNotification, build_witness_death_event, select_witnesses};

use tracing::{debug, warn};
use uuid::Uuid;

use crate::game_data::registry_or_error;
use crate::models::{AgentState, WorldEventType};
use cyber_jianghu_protocol::DeathInfo;

/// 应用生理值衰减和环境压力伤害
///
/// 生理值衰减逻辑由 StatusComponent 统一处理（基于配置），包括：
/// - 饱食度、饱饮度、体力等属性的自然变化
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
///
/// `acted_recently`：本 tick 窗口内提交过 intent 的 Agent 集合。
/// 集合外的 Agent 视为休息 tick（idle-skip/离线/思考间隙），
/// decay≠0 且有 recovery_formula 的属性（如 sanity）仅在休息 tick 恢复。
pub fn apply_decay_and_environmental_damage(
    tick_id: i64,
    mut agent_states: Vec<AgentState>,
    acted_recently: &std::collections::HashSet<Uuid>,
) -> DecayResult {
    let mut dead_agents = Vec::new();
    let mut events = Vec::new();
    let mut death_notifications = Vec::new();

    // 获取位置注册表
    let registry = match registry_or_error() {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("注册表未初始化: {}", e);
            return (agent_states, vec![], vec![], vec![]);
        }
    };
    let location_registry = registry.location_registry.read().expect("rwlock poisoned");

    for state in &mut agent_states {
        let was_alive = state.is_alive;
        let agent_id = state.agent_id;
        let location = state.node_id.clone();

        // 应用基础生理值衰减
        // 传递 tick_id，以便 apply_decay 可以获取季节信息
        // 休息判定：本 tick 窗口无 intent = 身体在休息（门控恢复生效）
        let rested = !acted_recently.contains(&agent_id);
        // 返回触发死亡的属性名（如果有）
        let death_attr_name = state.apply_decay_with_rest(tick_id, rested);

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

        // 寿终正寝检查（birth_tick 非空时生效，NULL = 不朽）
        if was_alive
            && state.is_alive
            && let Some(birth_tick) = state.birth_tick
            && birth_tick > 0
            && birth_tick < tick_id
        {
            // 复用 compute_game_time 相同公式，从秒级 tick_id 计算游戏年
            let age_years = compute_age_years(birth_tick, tick_id);
            if let Some((max_age, _aging_start, _starting_age)) = registry.get_lifespan_config()
                && age_years >= max_age as i64
            {
                // 寿终正寝：清零 HP
                state.status.set("hp", 0).ok();
                state.is_alive = false;
                dead_agents.push(agent_id);

                let death_info = registry.get_old_age_death_info();
                warn!(
                    "Agent {} 寿终正寝，享年 {} 岁（max_age={}）",
                    agent_id, age_years, max_age
                );

                let death_event = crate::models::WorldEvent {
                    event_type: WorldEventType::DeathNotification,
                    tick_id,
                    description: death_info.message.clone(),
                    metadata: serde_json::json!({
                        "cause": &death_info.cause,
                        "location": &state.node_id,
                        "age_years": age_years,
                    }),
                };
                events.push((agent_id, death_event));

                let notification = DeathNotification::new(
                    agent_id,
                    death_info.cause,
                    death_info.message,
                    state.node_id.clone(),
                    tick_id,
                );
                death_notifications.push(notification);
            }
        }
    }

    // 处理物品耐久度自然衰减
    // 异步操作需要 db_pool，这里先收集需要处理的物品 ID
    // 物品自然损坏（需耐久度系统支持）

    (agent_states, dead_agents, events, death_notifications)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_apply_decay_logic() {
        crate::game_data::init_test_registry();

        // 测试 AgentState 的衰减逻辑
        // 白板重生初始值：HP=100, 体力=100, 饥饿=50, 口渴=50
        let mut state = AgentState::new(uuid::Uuid::new_v4(), 1);
        assert_eq!(state.status.get("satiation").unwrap_or(0), 50);
        assert_eq!(state.status.get("hydration").unwrap_or(0), 50);
        assert_eq!(state.status.get("stamina").unwrap_or(0), 100);

        // 测试配置 decay_per_tick = 0.2（累计器）→ 单 tick 不扣减
        let _ = state.apply_decay(1);
        assert_eq!(state.status.get("satiation").unwrap_or(0), 50);
        assert_eq!(state.status.get("hydration").unwrap_or(0), 50);
        assert_eq!(state.status.get("stamina").unwrap_or(0), 100);

        // 跑满 5 tick 累计器到 -1.0，扣 1
        for _ in 0..4 {
            let _ = state.apply_decay(1);
        }
        assert_eq!(state.status.get("satiation").unwrap_or(0), 49);
        assert_eq!(state.status.get("hydration").unwrap_or(0), 49);
    }

    /// 休息门控恢复：休息 tick 恢复 sanity，行动 tick 只衰减
    #[test]
    fn test_rest_gated_recovery_differentiates_acted_and_rested() {
        crate::game_data::init_test_registry();

        // 测试 fixture 中 sanity 配置为 decay_per_tick=1 + recovery_formula="4"
        let mut rested_agent = AgentState::new(Uuid::new_v4(), 1);
        rested_agent.is_alive = true;
        rested_agent.status.set("sanity", 50).unwrap();
        let rested_id = rested_agent.agent_id;

        let mut acted_agent = AgentState::new(Uuid::new_v4(), 1);
        acted_agent.is_alive = true;
        acted_agent.status.set("sanity", 50).unwrap();
        let acted_id = acted_agent.agent_id;

        let acted_recently: std::collections::HashSet<Uuid> = [acted_id].into();

        let (updated_states, dead, _, _) = apply_decay_and_environmental_damage(
            1,
            vec![rested_agent, acted_agent],
            &acted_recently,
        );
        assert!(dead.is_empty());

        let by_id = |id: Uuid| updated_states.iter().find(|s| s.agent_id == id).unwrap();
        // 行动：仅 -1 衰减 = 49
        assert_eq!(by_id(acted_id).status.get("sanity"), Some(49));
        // 休息：-1 衰减 +4 恢复 = 53
        assert_eq!(by_id(rested_id).status.get("sanity"), Some(53));
    }
}
