// ============================================================================
// WorldState 构建器（自 broadcaster.rs 单文件拆分，原文件超 800 行上限）
// ============================================================================
//
// 三个入口共用 time.rs 的游戏时换算与邻接可见性过滤：
// - build_world_state_for_agent：tick 周期广播版（个性化全量）
// - build_reactive_world_state：Intent 执行后即时推送版
// - build_initial_world_state：Agent 连接/重连初始版（简化）
// ============================================================================

use std::collections::HashMap;
use uuid::Uuid;

use crate::models::{AgentState, WorldEvent, WorldEventType, WorldState};
use cyber_jianghu_protocol::{AdjacentNode, EVENT_TYPE_DEATH_NOTIFICATION, EVENT_TYPE_WORLD_STATE};

use super::recipes::build_recipe_details;
use super::time::compute_game_time;

/// 为单个Agent构建WorldState消息
///
/// 包含周围Agent信息（同节点、存活、在线的其他Agent）
/// 使用数据驱动：从配置加载位置信息和可用动作
#[allow(clippy::too_many_arguments)]
pub(super) fn build_world_state_for_agent(
    agent_state: &AgentState,
    tick_id: i64,
    mut events: Vec<WorldEvent>,
    all_agent_states: &[AgentState],
    agent_names: &HashMap<Uuid, String>,
    inventory: Vec<crate::models::InventoryItem>,
    nearby_items: Vec<cyber_jianghu_protocol::SceneItem>,
    online_agent_ids: &std::collections::HashSet<Uuid>,
    game_data: &crate::game_data::types::GameData,
    location_registry: &crate::game_data::LocationRegistry,
    recent_actions_map: &HashMap<Uuid, Vec<cyber_jianghu_protocol::RecentAction>>,
    emergence_config: &crate::game_data::types::unified_config::EmergenceConfig,
    known_recipe_ids: Option<&Vec<String>>,
) -> WorldState {
    // 游戏时间计算（数据驱动）
    let (year, month, day, hour) = compute_game_time(tick_id);

    // 获取当前Agent的node_id
    let current_node_id = &agent_state.node_id;

    // 位置信息和相邻节点
    let location_node = location_registry.get_node(current_node_id);

    // 获取位置名称和类型（数据驱动）
    let location_name = location_node
        .map(|n| n.name.clone())
        .unwrap_or_else(|| current_node_id.clone());

    let location_type = location_node
        .map(|n| format!("{:?}", n.node_type))
        .unwrap_or_else(|| "未知".to_string());

    // 获取相邻节点（数据驱动：显式边 + 隐式 parent-child）
    let default_implicit_cost = game_data
        .game_rules
        .data
        .agent_state
        .location
        .default_implicit_travel_cost;
    let adjacent_nodes: Vec<AdjacentNode> = location_registry.get_visible_neighbors(
        current_node_id,
        default_implicit_cost,
        crate::game_data::registry::time_registry::TimeRegistry::game_day(tick_id),
    );

    // 过滤events_log：只保留与当前Agent同节点的事件
    // 全局事件（如系统通知）没有location字段，会被保留
    events.retain(|e| {
        if let Some(loc) = e.metadata.get("location")
            && let Some(loc_str) = loc.as_str()
        {
            return loc_str == current_node_id;
        }
        true
    });

    // 如果 Agent 已经死亡，添加一个特殊的系统事件
    if !agent_state.is_alive {
        let has_death_event = events.iter().any(|e| {
            if let Some(cause) = e.metadata.get("cause")
                && let Some(cause_str) = cause.as_str()
            {
                return cause_str.starts_with("death");
            }
            false
        });

        if !has_death_event {
            let death_message = game_data.display_messages.notifications.death.clone();
            events.push(WorldEvent {
                event_type: WorldEventType::SystemNotification,
                tick_id,
                description: death_message,
                metadata: serde_json::json!({
                    "type": EVENT_TYPE_DEATH_NOTIFICATION,
                    "message": "You are dead.",
                }),
            });
        }
    }

    // 获取显示消息配置（数据驱动）
    let (entity_state_alive, entity_state_dead) = (
        game_data.display_messages.entity_states.alive.clone(),
        game_data.display_messages.entity_states.dead.clone(),
    );

    // 筛选同节点的其他存活且在线的Agent（排除自己）
    let entities: Vec<crate::models::Entity> = all_agent_states
        .iter()
        .filter(|other| {
            // 排除自己
            other.agent_id != agent_state.agent_id &&
                // 同一节点
                &other.node_id == current_node_id &&
                // 存活
                other.is_alive &&
                // 在线（WebSocket 已连接）
                online_agent_ids.contains(&other.agent_id)
        })
        .map(|other| {
            // 获取Agent名称
            let name = agent_names
                .get(&other.agent_id)
                .cloned()
                .unwrap_or_else(|| format!("Agent-{}", other.agent_id));

            // 检查是否死亡（通过hp属性）
            let is_dead = other.status.get("hp").map(|hp| hp <= 0).unwrap_or(false);

            crate::models::Entity {
                id: other.agent_id,
                name,
                distance: 0, // MVP阶段：同节点距离为0
                state: if is_dead {
                    entity_state_dead.clone()
                } else {
                    entity_state_alive.clone()
                },
                hostile: false, // MVP阶段：无敌对关系
                recent_actions: recent_actions_map
                    .get(&other.agent_id)
                    .map(|actions| {
                        actions
                            .iter()
                            .take(emergence_config.max_recent_actions_per_entity)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect();

    // 获取天气描述（数据驱动：季节 → weather_pool → display_messages）
    let weather = crate::game_data::registry::time_registry::TimeRegistry::get_weather(tick_id)
        .unwrap_or_else(|| game_data.display_messages.weather.sunny.clone());

    // 构建WorldState
    WorldState {
        event_type: EVENT_TYPE_WORLD_STATE.to_string(),
        tick_id,
        agent_id: Some(agent_state.agent_id),
        world_time: crate::models::WorldTime {
            year,
            month,
            day,
            hour,
            minute: 0,
            second: 0,
            weather,
        },
        location: crate::models::Location {
            node_id: current_node_id.clone(),
            name: location_name,
            node_type: location_type,
            parent_chain: location_registry.ancestor_names(current_node_id),
            adjacent_nodes,
            gatherable_items: location_node
                .map(|n| {
                    n.gatherable_items
                        .iter()
                        .filter_map(|id| {
                            crate::game_data::ItemRegistry::get(id).map(|entry| {
                                crate::models::GatherableItem {
                                    // 采集引用 uuid（与背包/地面物品同标识体系）
                                    item_id: crate::items::item_uuid(id).to_string(),
                                    name: entry.name.clone(),
                                    item_type: entry.item_type.clone(),
                                }
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        self_state: {
            // 获取属性数值
            let attributes = agent_state.get_attributes_for_protocol();

            // 获取派生属性（浮点数）
            let derived_attributes = agent_state.get_derived_attributes_for_protocol();

            // 从 NarrativeConfig 生成叙事描述（数据驱动：阈值描述→显示名回退）
            let attribute_descriptions = game_data
                .narrative
                .build_attribute_descriptions(&attributes, &derived_attributes);

            let survival_drives = game_data.narrative.compute_survival_drives(&attributes);
            crate::models::AgentSelfState {
                attributes,
                derived_attributes,
                attribute_descriptions,
                survival_drives,
                // 注意：status_effects 字段暂未实现，始终为空数组
                // Agent 的实际状态效果通过 attribute_descriptions 描述
                status_effects: vec![],
                inventory,
                skills: agent_state
                    .skills
                    .iter()
                    .filter_map(|skill_id| {
                        crate::game_data::registry::SkillRegistry::get(skill_id).map(|def| {
                            cyber_jianghu_protocol::types::entities::SkillInfo {
                                skill_id: skill_id.clone(),
                                name: def.name,
                            }
                        })
                    })
                    .collect(),
                // 寿命数据（由 Server 从 birth_tick + time.yaml 计算）
                age_years: agent_state
                    .birth_tick
                    .map(|bt| crate::tick::decay::compute_age_years(bt, tick_id) as u32),
                max_age: game_data
                    .game_rules
                    .data
                    .lifespan
                    .as_ref()
                    .map(|l| l.max_age as u32),
                recipe_details: build_recipe_details(
                    known_recipe_ids
                        .as_ref()
                        .map(|v| v.as_slice())
                        .unwrap_or(&[]),
                ),
            }
        },
        entities, // 包含同节点的其他Agent
        nearby_items,
        events_log: events,
        private_dialogue_log: vec![], // 实时模式：密语记录由 IntentWorker 即时处理
        last_execution_summary: None, // 实时模式：ExecutionResult 通过独立通道反馈
    }
}

/// 构建交互驱动 WorldState（Intent 执行后即时推送）
///
/// 与 tick 广播版相比：
/// - 包含 Intent 结果事件（events_log），使 Agent 能立即处理 SocialInteraction 等事件
/// - 包含同位置 entities（让 agent 看到其他 agent 的状态变化）
#[allow(clippy::too_many_arguments)]
pub fn build_reactive_world_state(
    agent_state: &AgentState,
    co_located_states: &[AgentState],
    tick_id: i64,
    inventory: &[crate::models::InventoryItem],
    nearby_items: &[cyber_jianghu_protocol::SceneItem],
    agent_names: &HashMap<Uuid, String>,
    online_ids: &std::collections::HashSet<Uuid>,
    game_data: &crate::game_data::GameData,
    location_registry: &crate::game_data::LocationRegistry,
    recent_actions_map: &HashMap<Uuid, Vec<cyber_jianghu_protocol::RecentAction>>,
    events: Vec<crate::models::WorldEvent>,
    recipe_details: Vec<cyber_jianghu_protocol::types::entities::RecipeDetail>,
) -> crate::models::WorldState {
    let (year, month, day, hour) = compute_game_time(tick_id);
    let current_node_id = &agent_state.node_id;

    // 位置信息
    let location_node = location_registry.get_node(current_node_id);
    let location_name = location_node
        .map(|n| n.name.clone())
        .unwrap_or_else(|| current_node_id.clone());
    let location_type = location_node
        .map(|n| format!("{:?}", n.node_type))
        .unwrap_or_else(|| "未知".to_string());
    let default_implicit_cost = game_data
        .game_rules
        .data
        .agent_state
        .location
        .default_implicit_travel_cost;
    let adjacent_nodes: Vec<AdjacentNode> = location_registry.get_visible_neighbors(
        current_node_id,
        default_implicit_cost,
        crate::game_data::registry::time_registry::TimeRegistry::game_day(tick_id),
    );
    let gatherable_items: Vec<crate::models::GatherableItem> = location_node
        .map(|n| {
            n.gatherable_items
                .iter()
                .filter_map(|id| {
                    crate::game_data::ItemRegistry::get(id).map(|entry| {
                        crate::models::GatherableItem {
                            item_id: id.clone(),
                            name: entry.name.clone(),
                            item_type: entry.item_type.clone(),
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // 显示消息配置
    let (entity_state_alive, entity_state_dead) = (
        game_data.display_messages.entity_states.alive.clone(),
        game_data.display_messages.entity_states.dead.clone(),
    );

    // 同位置 entities（排除自己、必须存活且在线）
    let entities: Vec<crate::models::Entity> = co_located_states
        .iter()
        .filter(|other| {
            other.agent_id != agent_state.agent_id
                && other.is_alive
                && online_ids.contains(&other.agent_id)
        })
        .map(|other| {
            let name = agent_names
                .get(&other.agent_id)
                .cloned()
                .unwrap_or_else(|| format!("Agent-{}", other.agent_id));
            let is_dead = other.status.get("hp").map(|hp| hp <= 0).unwrap_or(false);
            crate::models::Entity {
                id: other.agent_id,
                name,
                distance: 0,
                state: if is_dead {
                    entity_state_dead.clone()
                } else {
                    entity_state_alive.clone()
                },
                hostile: false,
                recent_actions: recent_actions_map
                    .get(&other.agent_id)
                    .map(|actions| actions.iter().take(3).cloned().collect())
                    .unwrap_or_default(),
            }
        })
        .collect();

    let weather = crate::game_data::registry::time_registry::TimeRegistry::get_weather(tick_id)
        .unwrap_or_else(|| game_data.display_messages.weather.sunny.clone());

    // 属性
    let attributes = agent_state.get_attributes_for_protocol();
    let derived_attributes = agent_state.get_derived_attributes_for_protocol();
    let attribute_descriptions = game_data
        .narrative
        .build_attribute_descriptions(&attributes, &derived_attributes);

    crate::models::WorldState {
        event_type: EVENT_TYPE_WORLD_STATE.to_string(),
        tick_id,
        agent_id: Some(agent_state.agent_id),
        world_time: crate::models::WorldTime {
            year,
            month,
            day,
            hour,
            minute: 0,
            second: 0,
            weather,
        },
        location: crate::models::Location {
            node_id: current_node_id.clone(),
            name: location_name,
            node_type: location_type,
            parent_chain: location_registry.ancestor_names(current_node_id),
            adjacent_nodes,
            gatherable_items,
        },
        self_state: {
            let survival_drives = game_data.narrative.compute_survival_drives(&attributes);
            crate::models::AgentSelfState {
                attributes,
                derived_attributes,
                attribute_descriptions,
                survival_drives,
                status_effects: vec![],
                inventory: inventory.to_vec(),
                skills: agent_state
                    .skills
                    .iter()
                    .filter_map(|skill_id| {
                        crate::game_data::registry::SkillRegistry::get(skill_id).map(|def| {
                            cyber_jianghu_protocol::types::entities::SkillInfo {
                                skill_id: skill_id.clone(),
                                name: def.name,
                            }
                        })
                    })
                    .collect(),
                age_years: agent_state.birth_tick.map(|bt| {
                    crate::tick::decay::compute_age_years(bt, agent_state.tick_id) as u32
                }),
                max_age: game_data
                    .game_rules
                    .data
                    .lifespan
                    .as_ref()
                    .map(|l| l.max_age as u32),
                recipe_details,
            }
        },
        entities,
        nearby_items: nearby_items.to_vec(),
        events_log: events, // Intent 结果事件（SocialInteraction 等）
        private_dialogue_log: vec![],
        last_execution_summary: None,
    }
}

/// 构建 Agent 连接时的初始 WorldState（简化版）
///
/// 不含其他 agent entities，用于让 agent 立即获知自身存活状态
/// `override_tick_id`: 如果提供，使用此 tick_id 而非 agent_state.tick_id（用于重连时同步到当前 tick）
pub fn build_initial_world_state(
    agent_state: &AgentState,
    game_data: &crate::game_data::GameData,
    location_registry: &crate::game_data::LocationRegistry,
    initial_inventory: Vec<crate::models::InventoryItem>,
    nearby_items: Vec<cyber_jianghu_protocol::SceneItem>,
    override_tick_id: Option<i64>,
    recipe_details: Vec<cyber_jianghu_protocol::types::entities::RecipeDetail>,
) -> crate::models::WorldState {
    let tick_id = override_tick_id.unwrap_or(agent_state.tick_id);

    // 游戏时间计算（与 build_world_state_for_agent 共用 compute_game_time）
    let (year, month, day, hour) = compute_game_time(tick_id);

    let current_node_id = &agent_state.node_id;

    // 位置信息
    let location_node = location_registry.get_node(current_node_id);
    let location_name = location_node
        .map(|n| n.name.clone())
        .unwrap_or_else(|| current_node_id.clone());
    let location_type = location_node
        .map(|n| format!("{:?}", n.node_type))
        .unwrap_or_else(|| "未知".to_string());
    let default_implicit_cost = game_data
        .game_rules
        .data
        .agent_state
        .location
        .default_implicit_travel_cost;
    let adjacent_nodes: Vec<AdjacentNode> = location_registry.get_visible_neighbors(
        current_node_id,
        default_implicit_cost,
        crate::game_data::registry::time_registry::TimeRegistry::game_day(tick_id),
    );
    let gatherable_items: Vec<crate::models::GatherableItem> = location_node
        .map(|n| {
            n.gatherable_items
                .iter()
                .filter_map(|id| {
                    crate::game_data::ItemRegistry::get(id).map(|entry| {
                        crate::models::GatherableItem {
                            item_id: id.clone(),
                            name: entry.name.clone(),
                            item_type: entry.item_type.clone(),
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // 死亡状态事件
    let mut events = Vec::new();
    if !agent_state.is_alive {
        let death_message = game_data.display_messages.notifications.death.clone();
        events.push(WorldEvent {
            event_type: WorldEventType::SystemNotification,
            tick_id,
            description: death_message,
            metadata: serde_json::json!({
                "type": EVENT_TYPE_DEATH_NOTIFICATION,
                "message": "You are dead.",
            }),
        });
    }

    let weather = crate::game_data::registry::time_registry::TimeRegistry::get_weather(tick_id)
        .unwrap_or_else(|| game_data.display_messages.weather.sunny.clone());

    // 属性
    let attributes = agent_state.get_attributes_for_protocol();
    let derived_attributes = agent_state.get_derived_attributes_for_protocol();
    let attribute_descriptions = game_data
        .narrative
        .build_attribute_descriptions(&attributes, &derived_attributes);

    let survival_drives = game_data.narrative.compute_survival_drives(&attributes);

    crate::models::WorldState {
        event_type: EVENT_TYPE_WORLD_STATE.to_string(),
        tick_id,
        agent_id: Some(agent_state.agent_id),
        world_time: crate::models::WorldTime {
            year,
            month,
            day,
            hour,
            minute: 0,
            second: 0,
            weather,
        },
        location: crate::models::Location {
            node_id: current_node_id.clone(),
            name: location_name,
            node_type: location_type,
            parent_chain: location_registry.ancestor_names(current_node_id),
            adjacent_nodes,
            gatherable_items,
        },
        self_state: crate::models::AgentSelfState {
            attributes,
            derived_attributes,
            attribute_descriptions,
            survival_drives,
            status_effects: vec![],
            inventory: initial_inventory,
            skills: agent_state
                .skills
                .iter()
                .filter_map(|skill_id| {
                    crate::game_data::registry::SkillRegistry::get(skill_id).map(|def| {
                        cyber_jianghu_protocol::types::entities::SkillInfo {
                            skill_id: skill_id.clone(),
                            name: def.name,
                        }
                    })
                })
                .collect(),
            age_years: agent_state
                .birth_tick
                .map(|bt| crate::tick::decay::compute_age_years(bt, agent_state.tick_id) as u32),
            max_age: game_data
                .game_rules
                .data
                .lifespan
                .as_ref()
                .map(|l| l.max_age as u32),
            recipe_details,
        },
        entities: vec![], // 连接时不含其他 agent
        nearby_items,
        events_log: events,
        private_dialogue_log: vec![],
        last_execution_summary: None,
    }
}
