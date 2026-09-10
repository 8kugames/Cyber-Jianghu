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
use crate::game_data::registry::ItemRegistry;
use crate::models::AgentState;
use crate::websocket::{AgentToDeviceMap, ConnectionManager, send_world_state};

mod recipes;
mod time;
mod world_state;

pub use recipes::build_recipe_details;
pub use world_state::{build_initial_world_state, build_reactive_world_state};

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
    /// 为每个在线Agent构建个性化WorldState并通过WebSocket发送。
    /// 离线Agent直接跳过（不构建、不发送，节省逐agent构建开销）；
    /// 死亡但在线的Agent不受影响（在 online 集合中），死亡通知依赖广播送达。
    #[allow(clippy::too_many_arguments)]
    pub async fn broadcast_states(
        &self,
        tick_id: i64,
        agent_states: &[AgentState],
        db_pool: &DbPool,
        connection_manager: &ConnectionManager,
        agent_to_device_map: &AgentToDeviceMap,
        event_manager: &super::event_manager::SharedEventManager,
        game_data_cache: &Arc<GameDataCache>,
    ) -> anyhow::Result<()> {
        use crate::db::get_all_agents;

        // 获取配置快照（owned Arc，Send-safe，避免 RwLockReadGuard 跨 .await）
        let gd = game_data_cache.snapshot();
        let loc_registry = game_data_cache.location_snapshot();

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
        // 注意：ConnectionManager 的 key 是 device_id，但我们需要 agent_id
        let online_agent_ids: std::collections::HashSet<Uuid> = {
            let connections = connection_manager.read().await;
            connections.values().map(|c| c.agent_id).collect()
        };

        // 批量加载所有 Agent 的背包（单次 DB 查询，解决 N+1 问题）
        // 只为在线 Agent 批量加载（离线 Agent 不会构建 WorldState）
        let agent_ids: Vec<Uuid> = agent_states
            .iter()
            .filter(|s| online_agent_ids.contains(&s.agent_id))
            .map(|s| s.agent_id)
            .collect();
        let agent_inventories = match crate::inventory::InventoryManager::get_all_items_batch(
            db_pool, &agent_ids,
        )
        .await
        {
            Ok(batch) => {
                let mut map: HashMap<Uuid, Vec<crate::models::InventoryItem>> = HashMap::new();
                for (agent_id, items) in batch {
                    let proto_items: Vec<crate::models::InventoryItem> = items
                        .into_iter()
                        .map(|item| {
                            let config = ItemRegistry::get(&item.item_id);
                            let name = config
                                .as_ref()
                                .map(|c| c.name.clone())
                                .unwrap_or_else(|| item.item_id.clone());
                            let item_type = config
                                .as_ref()
                                .map(|c| c.item_type.clone())
                                .unwrap_or_default();
                            crate::models::InventoryItem {
                                // 协议层携带物品 uuid（v5 派生）：Agent 按 uuid 引用物品，
                                // Server 在动作边界反解回内部 item_id
                                item_id: crate::items::item_uuid(&item.item_id).to_string(),
                                name,
                                quantity: item.quantity,
                                is_equipped: item.is_equipped,
                                item_type,
                            }
                        })
                        .collect();
                    map.insert(agent_id, proto_items);
                }
                map
            }
            Err(e) => {
                warn!("批量加载背包失败: {}", e);
                HashMap::new()
            }
        };

        // 批量加载所有 Agent 的已知配方（单次 DB 查询）
        let recipe_ids_map = match crate::db::batch_get_known_recipe_ids(db_pool, &agent_ids).await
        {
            Ok(map) => map,
            Err(e) => {
                warn!("批量加载配方失败: {}", e);
                HashMap::new()
            }
        };

        // 批量加载所有节点的地面物品（单次 DB 查询）；按在线集合过滤，与 agent_ids 对称
        let node_ids: Vec<String> = agent_states
            .iter()
            .filter(|s| online_agent_ids.contains(&s.agent_id))
            .map(|s| s.node_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let ground_items_map = match crate::db::get_ground_items_by_nodes(db_pool, &node_ids).await
        {
            Ok(map) => map,
            Err(e) => {
                warn!("批量加载地面物品失败: {}", e);
                HashMap::new()
            }
        };

        // 涌现：批量加载近期动作历史
        let (emergence_config, tick_duration_secs) = {
            let ec = gd.game_rules.data.emergence.clone().unwrap_or_default();
            let td = gd.game_rules.data.agent_state.tick.real_seconds_per_tick as i64;
            (ec, td)
        };
        let recent_actions_map = if emergence_config.recent_action_ticks > 0 {
            // tick_id 按 tick_duration_secs 递增，需要乘以 tick 间隔来计算 since_tick
            let since_tick = tick_id - emergence_config.recent_action_ticks * tick_duration_secs;
            info!(
                "涌现加载: tick={}, since_tick={}, agent_count={}, max_per_entity={}",
                tick_id,
                since_tick,
                agent_ids.len(),
                emergence_config.max_recent_actions_per_entity
            );
            match crate::db::get_recent_actions_batch(
                db_pool,
                &agent_ids,
                since_tick,
                emergence_config.max_recent_actions_per_entity,
            )
            .await
            {
                Ok(map) => {
                    info!("涌现加载完成: {} 个 agent 有动作记录", map.len());
                    map
                }
                Err(e) => {
                    warn!("批量加载近期动作失败: {}", e);
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        // 为每个Agent构建个性化WorldState并发送
        let mut sent_count = 0;

        // （已移除）跨 Agent 传承 Layer 2 教训批量加载：死亡知识不聚合广播，
        // 传播为纯涌现（目击 → 记忆 → 自主"说" → 扩散）

        for agent_state in agent_states {
            // 离线 Agent 跳过：无 WebSocket 连接，构建了也发不出去。
            // 实体可见性已按 online 集合过滤，对在线 Agent 的世界视图零影响。
            if !online_agent_ids.contains(&agent_state.agent_id) {
                continue;
            }
            let events = event_manager
                .lock()
                .expect("lock poisoned")
                .get_events_for_agent(agent_state.agent_id);
            let inventory = agent_inventories
                .get(&agent_state.agent_id)
                .cloned()
                .unwrap_or_default();

            let nearby_items = ground_items_map
                .get(&agent_state.node_id)
                .map(|items| {
                    items
                        .iter()
                        .map(|gi| {
                            let config = ItemRegistry::get(&gi.item_id);
                            let name = config
                                .as_ref()
                                .map(|c| c.name.clone())
                                .unwrap_or_else(|| gi.item_id.clone());
                            let item_type = config
                                .as_ref()
                                .map(|c| c.item_type.clone())
                                .unwrap_or_default();
                            cyber_jianghu_protocol::SceneItem {
                                item_id: crate::items::item_uuid(&gi.item_id).to_string(),
                                name,
                                quantity: gi.quantity,
                                item_type,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            let world_state = world_state::build_world_state_for_agent(
                agent_state,
                tick_id,
                events,
                agent_states,
                &agent_names,
                inventory,
                nearby_items,
                &online_agent_ids,
                &gd,
                &loc_registry,
                &recent_actions_map,
                &emergence_config,
                recipe_ids_map.get(&agent_state.agent_id),
            );

            // 向该Agent发送其专属的WorldState
            if let Err(e) = send_world_state(
                agent_state.agent_id,
                world_state,
                connection_manager,
                agent_to_device_map,
            )
            .await
            {
                warn!("向Agent {} 发送WorldState失败: {}", agent_state.agent_id, e);
            } else {
                sent_count += 1;
            }
        }

        info!("向 {} 个Agent发送了个性化WorldState", sent_count);
        Ok(())
    }
}

/// 向指定 agent 发送任意 ServerMessage
///
/// 通用单播函数，通过 agent_id → device_id → WebSocket 连接 发送消息。
/// 用于 tick processor 的验证错误通知等场景。
pub async fn send_to_agent(
    agent_id: Uuid,
    msg: &cyber_jianghu_protocol::ServerMessage,
    connection_manager: &ConnectionManager,
    agent_to_device_map: &AgentToDeviceMap,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let device_id = {
        let agent_to_device = agent_to_device_map.read().await;
        match agent_to_device.get(&agent_id) {
            Some(&device_id) => device_id,
            None => return Ok(()), // agent 不在线，静默跳过
        }
    };

    let mut connections = connection_manager.write().await;
    if let Some(connection) = connections.get_mut(&device_id) {
        if connection.is_dead() {
            return Ok(());
        }
        let json = serde_json::to_string(msg)?;
        let _ = connection
            .send(axum::extract::ws::Message::Text(json.into()))
            .await;
    }
    Ok(())
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new()
    }
}
