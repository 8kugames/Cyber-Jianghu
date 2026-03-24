// ============================================================================
// OpenClaw Cyber-Jianghu MVP Broadcaster
// ============================================================================
//
// 广播器负责向所有Agent广播新的世界状态，包括：
// 1. 为每个Agent构建个性化WorldState
// 2. 通过WebSocket发送WorldState
// 3. 计算游戏时间和周围实体
//
// 设计原则：
// 1. 每个Agent收到个性化的WorldState
// 2. 包含同节点的其他Agent信息
// 3. 包含本Tick发生的事件
// 4. 数据驱动：从配置加载动作、位置和显示消息
// ============================================================================

use anyhow::Context;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::db::DbPool;
use crate::game_data::GameDataCache;
use crate::game_data::registry::{ActionRegistry, ItemRegistry};
use crate::models::{AgentState, WorldEvent, WorldState};
use crate::websocket::{AgentToDeviceMap, ConnectionManager, send_world_state};
use cyber_jianghu_protocol::AdjacentNode;

use super::event_manager::EventManager;

/// 广播器
///
/// 负责向所有Agent广播新的世界状态
pub struct Broadcaster;

impl Broadcaster {
    /// 创建新的广播器
    pub fn new() -> Self {
        Self
    }

    /// 广播新状态给所有Agent
    ///
    /// 为每个Agent构建个性化WorldState并通过WebSocket发送
    pub async fn broadcast_states(
        &self,
        tick_id: i64,
        agent_states: &[AgentState],
        db_pool: &DbPool,
        connection_manager: &ConnectionManager,
        agent_to_device_map: &AgentToDeviceMap,
        event_manager: &EventManager,
        game_data_cache: &Arc<GameDataCache>,
    ) -> anyhow::Result<()> {
        use crate::db::get_all_agents;

        // 获取所有Agent的基本信息（用于构建entities）
        let all_agents = get_all_agents(db_pool)
            .await
            .context("获取所有Agent信息失败")?;

        // 构建Agent ID到名称的映射
        let agent_names: HashMap<Uuid, String> = all_agents
            .into_iter()
            .map(|agent| (agent.agent_id, agent.name))
            .collect();

        // 获取当前在线的 Agent ID 集合
        let online_agent_ids: std::collections::HashSet<Uuid> = {
            let connections = connection_manager.read().await;
            connections.keys().copied().collect()
        };

        // 批量加载所有 Agent 的背包
        let mut agent_inventories = HashMap::new();
        for agent_state in agent_states {
            match crate::inventory::InventoryManager::get_all_items(db_pool, agent_state.agent_id)
                .await
            {
                Ok(items) => {
                    // 转换为 protocol::InventoryItem
                    let proto_items: Vec<crate::models::InventoryItem> = items
                        .into_iter()
                        .map(|item| {
                            let name = ItemRegistry::get(&item.item_id)
                                .map(|config| config.name)
                                .unwrap_or_else(|| item.item_id.clone());

                            crate::models::InventoryItem {
                                item_id: item.item_id.clone(),
                                name,
                                quantity: item.quantity,
                                is_equipped: item.is_equipped,
                            }
                        })
                        .collect();
                    agent_inventories.insert(agent_state.agent_id, proto_items);
                }
                Err(e) => {
                    warn!("加载 Agent {} 背包失败: {}", agent_state.agent_id, e);
                }
            }
        }

        // 为每个Agent构建个性化WorldState并发送
        let mut sent_count = 0;
        for agent_state in agent_states {
            let events = event_manager.get_events_for_agent(agent_state.agent_id);
            let inventory = agent_inventories
                .get(&agent_state.agent_id)
                .cloned()
                .unwrap_or_default();

            let world_state = self.build_world_state_for_agent(
                agent_state,
                tick_id,
                events,
                agent_states,
                &agent_names,
                inventory,
                &online_agent_ids,
                game_data_cache,
            );

            // 向该Agent发送其专属的WorldState
            if let Err(e) =
                send_world_state(agent_state.agent_id, world_state, connection_manager, agent_to_device_map).await
            {
                warn!("向Agent {} 发送WorldState失败: {}", agent_state.agent_id, e);
            } else {
                sent_count += 1;
            }
        }

        info!("向 {} 个Agent发送了个性化WorldState", sent_count);
        Ok(())
    }

    /// 为单个Agent构建WorldState消息
    ///
    /// 包含周围Agent信息（同节点、存活、在线的其他Agent）
    /// 使用数据驱动：从配置加载位置信息和可用动作
    fn build_world_state_for_agent(
        &self,
        agent_state: &AgentState,
        tick_id: i64,
        mut events: Vec<WorldEvent>,
        all_agent_states: &[AgentState],
        agent_names: &HashMap<Uuid, String>,
        inventory: Vec<crate::models::InventoryItem>,
        online_agent_ids: &std::collections::HashSet<Uuid>,
        game_data_cache: &Arc<GameDataCache>,
    ) -> WorldState {
        // 计算游戏时间（每Tick = 1游戏小时）
        // 1 year = 12 months = 360 days = 8640 hours
        // 1 month = 30 days = 720 hours
        // 1 day = 24 hours
        let total_hours = tick_id;
        let year = 1 + (total_hours / 8640) as i32;
        let remaining_after_year = total_hours % 8640;
        let month = 1 + (remaining_after_year / 720) as i32;
        let remaining_after_month = remaining_after_year % 720;
        let day = 1 + (remaining_after_month / 24) as i32;
        let hour = (remaining_after_month % 24) as i32;

        // 获取当前Agent的node_id
        let current_node_id = &agent_state.node_id;

        // 从 GameDataCache 获取位置信息和相邻节点
        let location_registry = game_data_cache.location_registry.read().unwrap();
        let location_node = location_registry.get_node(current_node_id);

        // 获取位置名称和类型（数据驱动）
        let location_name = location_node
            .map(|n| n.name.clone())
            .unwrap_or_else(|| current_node_id.clone());

        let location_type = location_node
            .map(|n| format!("{:?}", n.node_type))
            .unwrap_or_else(|| "未知".to_string());

        // 获取相邻节点（数据驱动）
        let adjacent_nodes: Vec<AdjacentNode> = location_registry
            .get_neighbors(current_node_id)
            .iter()
            .filter_map(|edge| {
                location_registry.get_node(&edge.to_node_id).map(|node| {
                    AdjacentNode {
                        node_id: edge.to_node_id.clone(),
                        name: node.name.clone(),
                        travel_cost: edge.travel_cost,
                    }
                })
            })
            .collect();

        // 如果 Agent 已经死亡，添加一个特殊的系统事件
        if !agent_state.is_alive {
            let has_death_event = events.iter().any(|e| {
                if let Some(cause) = e.metadata.get("cause")
                    && let Some(cause_str) = cause.as_str() {
                        return cause_str.starts_with("death");
                    }
                false
            });

            if !has_death_event {
                let death_message = game_data_cache.get().display_messages.notifications.death.clone();
                events.push(WorldEvent {
                    event_type: "system_notification".to_string(),
                    tick_id,
                    description: death_message,
                    metadata: serde_json::json!({
                        "type": "death_notification",
                        "message": "You are dead.",
                    }),
                });
            }
        }

        // 获取显示消息配置（数据驱动）
        let (entity_state_alive, entity_state_dead) = {
            let gd = game_data_cache.get();
            (
                gd.display_messages.entity_states.alive.clone(),
                gd.display_messages.entity_states.dead.clone(),
            )
        };

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
                }
            })
            .collect();

        // 从 ActionRegistry 获取所有可用动作（数据驱动）
        let available_actions: Vec<crate::models::AvailableAction> = ActionRegistry::all_action_names()
            .into_iter()
            .filter_map(|action_name| {
                ActionRegistry::get(&action_name).map(|config| {
                    crate::models::AvailableAction {
                        action: action_name,
                        description: config.description,
                        valid_targets: None,
                    }
                })
            })
            .collect();

        // 获取天气描述（数据驱动，目前固定晴天）
        let weather = game_data_cache.get().display_messages.weather.sunny.clone();

        // 构建WorldState
        WorldState {
            event_type: "world_state".to_string(),
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
                adjacent_nodes,
            },
            self_state: {
                // 获取属性数值
                let attributes = agent_state.get_attributes_for_protocol();

                // 从 NarrativeConfig 生成叙事描述
                let game_data = game_data_cache.get();
                let attribute_descriptions: HashMap<String, String> = attributes
                    .iter()
                    .filter_map(|(name, &value)| {
                        game_data.narrative.get_description(name, value)
                            .map(|desc| (name.clone(), desc.to_string()))
                    })
                    .collect();

                crate::models::AgentSelfState {
                    attributes,
                    attribute_descriptions,
                    status_effects: vec![], // TODO: 从 Agent 状态加载
                    inventory,
                }
            },
            entities,             // 包含同节点的其他Agent
            nearby_items: vec![], // TODO: 添加场景物品
            events_log: events,   // 传递本 Tick 发生的事件
            available_actions,
        }
    }
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new()
    }
}
