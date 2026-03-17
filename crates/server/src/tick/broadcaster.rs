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
// ============================================================================

use anyhow::Context;
use std::collections::HashMap;
use tracing::{info, warn};
use uuid::Uuid;

use crate::db::DbPool;
use crate::models::{AgentState, WorldEvent, WorldState};
use crate::websocket::{send_world_state, ConnectionManager};

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
        event_manager: &EventManager,
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

        // 批量加载所有 Agent 的背包
        let mut agent_inventories = HashMap::new();
        for agent_state in agent_states {
            match crate::inventory::InventoryManager::get_all_items(db_pool, agent_state.agent_id).await {
                Ok(items) => {
                    // 转换为 protocol::InventoryItem
                    let proto_items: Vec<crate::models::InventoryItem> = items.into_iter().map(|item| {
                        crate::models::InventoryItem {
                            item_id: item.item_id.clone(),
                            name: item.item_id, // MVP: use ID as name if not available
                            quantity: item.quantity,
                            is_equipped: item.is_equipped,
                        }
                    }).collect();
                    agent_inventories.insert(agent_state.agent_id, proto_items);
                },
                Err(e) => {
                    warn!("加载 Agent {} 背包失败: {}", agent_state.agent_id, e);
                }
            }
        }

        // 为每个Agent构建个性化WorldState并发送
        let mut sent_count = 0;
        for agent_state in agent_states {
            let events = event_manager.get_events_for_agent(agent_state.agent_id);
            let inventory = agent_inventories.get(&agent_state.agent_id).cloned().unwrap_or_default();
            
            let world_state = self.build_world_state_for_agent(
                agent_state,
                tick_id,
                events,
                agent_states,
                &agent_names,
                inventory,
            );

            // 向该Agent发送其专属的WorldState
            if let Err(e) =
                send_world_state(agent_state.agent_id, world_state, connection_manager).await
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
    /// 包含周围Agent信息（同节点、存活的其他Agent）
    fn build_world_state_for_agent(
        &self,
        agent_state: &AgentState,
        tick_id: i64,
        mut events: Vec<WorldEvent>,
        all_agent_states: &[AgentState],
        agent_names: &HashMap<Uuid, String>,
        inventory: Vec<crate::models::InventoryItem>,
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

        // 如果 Agent 已经死亡，添加一个特殊的系统事件（如果本 Tick 没有具体的死亡事件）
        if !agent_state.is_alive {
            let has_death_event = events.iter().any(|e| {
                if let Some(cause) = e.metadata.get("cause") {
                    if let Some(cause_str) = cause.as_str() {
                        return cause_str.starts_with("death");
                    }
                }
                false
            });

            if !has_death_event {
                events.push(WorldEvent {
                    event_type: "system_notification".to_string(),
                    tick_id,
                    description: "你已经死亡。".to_string(),
                    metadata: serde_json::json!({
                        "type": "death_notification",
                        "message": "You are dead.",
                    }),
                });
            }
        }

        // 筛选同节点的其他存活Agent（排除自己）
        let entities: Vec<crate::models::Entity> = all_agent_states
            .iter()
            .filter(|other| {
                // 排除自己
                other.agent_id != agent_state.agent_id &&
                // 同一节点
                &other.node_id == current_node_id &&
                // 存活
                other.is_alive
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
                        "死亡".to_string()
                    } else {
                        "存活".to_string()
                    },
                    hostile: false, // MVP阶段：无敌对关系
                }
            })
            .collect();

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
                weather: "晴".to_string(),
            },
            location: crate::models::Location {
                node_id: current_node_id.clone(),
                name: current_node_id.clone(), // MVP阶段：使用node_id作为名称
                node_type: "客栈".to_string(), // MVP阶段：固定类型
            },
            self_state: crate::models::AgentSelfState {
                attributes: agent_state.get_attributes_for_protocol(),
                status_effects: vec![],
                inventory, 
            },
            entities,             // 包含同节点的其他Agent
            nearby_items: vec![], // TODO: 添加场景物品
            events_log: events,   // 传递本 Tick 发生的事件
            available_actions: vec![
                crate::models::AvailableAction {
                    action: "idle".to_string(),
                    description: "休息".to_string(),
                    valid_targets: None,
                },
                crate::models::AvailableAction {
                    action: "speak".to_string(),
                    description: "公开说话".to_string(),
                    valid_targets: None,
                },
            ],
        }
    }
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new()
    }
}
