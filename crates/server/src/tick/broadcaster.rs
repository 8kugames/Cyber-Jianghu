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
use crate::models::{AgentState, WorldEvent, WorldEventType, WorldState};
use crate::websocket::{AgentToDeviceMap, ConnectionManager, send_world_state};
use cyber_jianghu_protocol::{AdjacentNode, EVENT_TYPE_DEATH_NOTIFICATION, EVENT_TYPE_WORLD_STATE};

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
        let agent_ids: Vec<Uuid> = agent_states.iter().map(|s| s.agent_id).collect();
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
                                item_id: item.item_id.clone(),
                                name,
                                quantity: item.quantity,
                                is_equipped: item.is_equipped,
                                item_type,
                                aliases: config
                                    .as_ref()
                                    .map(|c| c.aliases.clone())
                                    .unwrap_or_default(),
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

        // 批量加载所有节点的地面物品（单次 DB 查询）
        let node_ids: Vec<String> = agent_states
            .iter()
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

        // 跨 Agent 传承 Layer 2: 批量加载教训（所有 Agent 共享同一份）
        let lessons =
            {
                let (threshold, limit) = gd.game_rules.data.lesson.as_ref()
                .map(|c| (c.threshold, c.max_broadcast))
                .unwrap_or((
                    crate::game_data::types::unified_config::LessonConfig::DEFAULT_THRESHOLD,
                    crate::game_data::types::unified_config::LessonConfig::DEFAULT_MAX_BROADCAST,
                ));
                super::lessons::fetch_lessons_for_broadcast(db_pool, threshold, limit).await
            };

        for agent_state in agent_states {
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
                                item_id: gi.item_id.clone(),
                                name,
                                quantity: gi.quantity,
                                item_type,
                                aliases: config
                                    .as_ref()
                                    .map(|c| c.aliases.clone())
                                    .unwrap_or_default(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            let mut world_state = self.build_world_state_for_agent(
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

            // 注入教训（所有 Agent 共享）
            if !lessons.is_empty() {
                world_state.lessons_learned = lessons.clone();
            }

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

    /// 为单个Agent构建WorldState消息
    ///
    /// 包含周围Agent信息（同节点、存活、在线的其他Agent）
    /// 使用数据驱动：从配置加载位置信息和可用动作
    #[allow(clippy::too_many_arguments)]
    fn build_world_state_for_agent(
        &self,
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
        let adjacent_nodes: Vec<AdjacentNode> =
            location_registry.get_all_neighbors(current_node_id, default_implicit_cost);

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
                adjacent_nodes,
                gatherable_items: location_node
                    .map(|n| {
                        n.gatherable_items
                            .iter()
                            .filter_map(|id| {
                                crate::game_data::ItemRegistry::get(id).map(|entry| {
                                    crate::models::GatherableItem {
                                        item_id: id.clone(),
                                        name: entry.name.clone(),
                                        item_type: entry.item_type.clone(),
                                        aliases: entry.aliases.clone(),
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

                // 从 NarrativeConfig 生成叙事描述
                let attribute_descriptions: HashMap<String, String> = attributes
                    .iter()
                    .filter_map(|(name, &value)| {
                        game_data
                            .narrative
                            .get_description(name, value)
                            .map(|desc| (name.clone(), desc.to_string()))
                    })
                    .collect();

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
                        .map(|bt| super::decay::compute_age_years(bt, tick_id) as u32),
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
            lessons_learned: vec![],      // 广播路径通过外层赋值注入
        }
    }
}

/// 从已知配方 ID 列表构建 RecipeDetail 列表
pub fn build_recipe_details(
    known_recipe_ids: &[String],
) -> Vec<cyber_jianghu_protocol::types::entities::RecipeDetail> {
    known_recipe_ids
        .iter()
        .filter_map(|recipe_id| {
            let recipe = crate::game_data::registry::RecipeRegistry::get(recipe_id)?;
            let result_item_config =
                crate::game_data::registry::ItemRegistry::get(&recipe.result_item);
            let materials: Vec<cyber_jianghu_protocol::types::entities::RecipeMaterialInfo> =
                recipe
                    .materials
                    .iter()
                    .map(|m| {
                        let item_config = crate::game_data::registry::ItemRegistry::get(&m.item_id);
                        cyber_jianghu_protocol::types::entities::RecipeMaterialInfo {
                            item_id: m.item_id.clone(),
                            item_name: item_config
                                .as_ref()
                                .map(|c| c.name.clone())
                                .unwrap_or_else(|| m.item_id.clone()),
                            quantity: m.quantity,
                        }
                    })
                    .collect();
            Some(cyber_jianghu_protocol::types::entities::RecipeDetail {
                recipe_id: recipe_id.clone(),
                name: recipe.name,
                description: recipe.description,
                materials,
                result_item: recipe.result_item.clone(),
                result_item_name: result_item_config
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| recipe.result_item.clone()),
                result_quantity: recipe.result_quantity,
                stamina_cost: recipe.stamina_cost,
            })
        })
        .collect()
}

/// 从 tick_id（秒数）计算游戏时间
///
/// 数据驱动：从 TimeRegistry 和 GameRules 读取时间参数
/// 返回 (year, month, day, hour)
fn compute_game_time(tick_id: i64) -> (i32, i32, i32, i32) {
    let time_config = crate::game_data::registry::TimeRegistry::get_config();
    if let Some(config) = time_config {
        let registry = match crate::game_data::registry_or_error() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("注册表未初始化，使用默认时间: {}", e);
                return (1, 1, 1, 0);
            }
        };
        let real_seconds_per_tick = registry
            .get()
            .game_rules
            .data
            .agent_state
            .tick
            .real_seconds_per_tick as i64;
        let ticks_per_hour = config.ticks_per_hour as i64;
        let hours_per_day = config.hours_per_day as i64;
        let days_per_season = config.days_per_season as i64;
        let seasons_per_year = config.seasons_per_year as i64;
        let days_per_year = seasons_per_year * days_per_season;

        let real_seconds_per_game_hour = real_seconds_per_tick * ticks_per_hour;
        let game_hours = if real_seconds_per_game_hour > 0 {
            tick_id / real_seconds_per_game_hour
        } else {
            tick_id
        };

        let hours_per_year = days_per_year * hours_per_day;
        let hours_per_month = days_per_season * hours_per_day;

        let year = 1 + (game_hours / hours_per_year) as i32;
        let rem_after_year = game_hours % hours_per_year;
        let month = 1 + (rem_after_year / hours_per_month) as i32;
        let rem_after_month = rem_after_year % hours_per_month;
        let day = 1 + (rem_after_month / hours_per_day) as i32;
        let hour = (rem_after_month % hours_per_day) as i32;

        (year, month, day, hour)
    } else {
        (1, 1, 1, 0)
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
    let adjacent_nodes: Vec<AdjacentNode> =
        location_registry.get_all_neighbors(current_node_id, default_implicit_cost);
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
                            aliases: entry.aliases.clone(),
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
    let attribute_descriptions: HashMap<String, String> = attributes
        .iter()
        .filter_map(|(name, &value)| {
            game_data
                .narrative
                .get_description(name, value)
                .map(|desc| (name.clone(), desc.to_string()))
        })
        .collect();

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
                age_years: agent_state
                    .birth_tick
                    .map(|bt| super::decay::compute_age_years(bt, agent_state.tick_id) as u32),
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
        lessons_learned: vec![], // 响应式 WorldState 不含教训
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
    let adjacent_nodes: Vec<AdjacentNode> =
        location_registry.get_all_neighbors(current_node_id, default_implicit_cost);
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
                            aliases: entry.aliases.clone(),
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
    let attribute_descriptions: HashMap<String, String> = attributes
        .iter()
        .filter_map(|(name, &value)| {
            game_data
                .narrative
                .get_description(name, value)
                .map(|desc| (name.clone(), desc.to_string()))
        })
        .collect();
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
                .map(|bt| super::decay::compute_age_years(bt, agent_state.tick_id) as u32),
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
        lessons_learned: vec![], // 初始 WorldState 不含教训
    }
}
