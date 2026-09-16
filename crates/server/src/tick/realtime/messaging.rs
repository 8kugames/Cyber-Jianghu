// ============================================================================
// 下行消息（执行结果/错误/reactive 推送/事件广播/死亡处理）
// ============================================================================

use std::collections::{HashMap, HashSet};
use tracing::{debug, info, warn};

use uuid::Uuid;

use crate::game_data::registry::{ActionRegistry, ItemRegistry};
use crate::governance::ServerGovernanceMapper;
use crate::models::{AgentState, WorldEvent};
use crate::tick::decay;
use crate::websocket::{DeathNotificationContext, send_agent_died_notification};

use super::IntentWorker;
use anyhow::Result;

impl IntentWorker {
    /// 发送 ExecutionResult 给指定 Agent
    #[allow(clippy::too_many_arguments)]
    /// 推送最新动作配置给指定 Agent（UnknownAction 拒绝的自愈链路）
    ///
    /// Agent 側 action_update 回调收到后刷新引擎词表（prompt 动作索引 + chaos
    /// 候选），使下一次决策即使用有效动作词汇。
    pub(super) async fn push_fresh_actions(&self, agent_id: uuid::Uuid, tick_id: i64) {
        let available_actions = ActionRegistry::build_available_actions();
        let msg = cyber_jianghu_protocol::ServerMessage::config_update_full_value(
            cyber_jianghu_protocol::ConfigType::Actions,
            format!("unknown-action-heal-{tick_id}"),
            serde_json::to_value(&available_actions).unwrap_or(serde_json::Value::Array(vec![])),
            None,
        );
        if let Err(e) = super::send_to_agent(
            agent_id,
            &msg,
            &self.connection_manager,
            &self.agent_to_device_map,
        )
        .await
        {
            debug!(
                "UnknownAction 自愈推送失败（agent 可能离线）: agent={}, error={}",
                agent_id, e
            );
        }
    }

    #[allow(clippy::too_many_arguments)] // 执行结果字段天然与 ExecutionResult 载荷一一对应，结构体化反增样板
    pub(super) async fn send_execution_result(
        &self,
        agent_id: uuid::Uuid,
        intent_id: uuid::Uuid,
        tick_id: i64,
        success: bool,
        error: Option<String>,
        state_change_summary: Option<String>,
        governance_code: Option<cyber_jianghu_protocol::GovernanceCode>,
    ) {
        let msg = cyber_jianghu_protocol::ServerMessage::ExecutionResult {
            tick_id,
            intent_id,
            success,
            error,
            state_change_summary,
            governance_code,
        };
        if let Err(e) = super::send_to_agent(
            agent_id,
            &msg,
            &self.connection_manager,
            &self.agent_to_device_map,
        )
        .await
        {
            debug!(
                "ExecutionResult 发送失败: agent={}, intent={}, error={}",
                agent_id, intent_id, e
            );
        }
    }

    /// 发送错误给指定 Agent（封装为失败的 ExecutionResult）
    pub(super) async fn send_error_to_agent(
        &self,
        agent_id: uuid::Uuid,
        intent_id: uuid::Uuid,
        _code: &str,
        message: &str,
        tick_id: i64,
    ) {
        let governance_code = ServerGovernanceMapper::map_from_error(message);
        self.send_execution_result(
            agent_id,
            intent_id,
            tick_id,
            false,
            Some(message.to_string()),
            None,
            Some(governance_code),
        )
        .await;
    }

    /// 交互驱动即时推送 WorldState
    ///
    /// Intent 执行 / 死亡善后等事件后，为同位置在线 Agent 构建并发送最新 WorldState。
    /// 确保 Agent 在下一次认知决策前拥有最新的世界状态与事件流
    /// （events_log → 记忆 + 特质演化的规范通道）。
    pub(super) async fn send_reactive_world_state(
        &self,
        location: &str,
        tick_id: i64,
        events: Vec<WorldEvent>,
    ) {
        let location = location.to_string();

        // 收集同位置存活 Agent（发起者状态刚写入缓存，天然包含自身）
        let co_located: Vec<AgentState> = self
            .state_cache
            .iter()
            .filter(|r| r.value().node_id == location && r.value().is_alive)
            .map(|r| r.value().clone())
            .collect();

        let co_located_ids: Vec<Uuid> = co_located.iter().map(|s| s.agent_id).collect();

        // 3. 批量加载所需数据
        let agent_names = match crate::db::get_all_agents(&self.db_pool).await {
            Ok(agents) => agents
                .into_iter()
                .map(|a| (a.agent_id, a.name))
                .collect::<HashMap<Uuid, String>>(),
            Err(e) => {
                warn!("reactive WorldState: 加载 agent 名称失败: {}", e);
                return;
            }
        };

        let inventories = match crate::inventory::InventoryManager::get_all_items_batch(
            &self.db_pool,
            &co_located_ids,
        )
        .await
        {
            Ok(batch) => batch,
            Err(e) => {
                warn!("reactive WorldState: 加载背包失败: {}", e);
                HashMap::new()
            }
        };

        let ground_items = match crate::db::get_ground_items_by_nodes(
            &self.db_pool,
            std::slice::from_ref(&location),
        )
        .await
        {
            Ok(map) => map,
            Err(e) => {
                warn!("reactive WorldState: 加载地面物品失败: {}", e);
                HashMap::new()
            }
        };

        // 在线状态
        let online_ids: HashSet<Uuid> = {
            let connections = self.connection_manager.read().await;
            connections.values().map(|c| c.agent_id).collect()
        };

        // 3.5 加载同位置 Agent 的 recent_actions（让社交因果链闭合）
        let tick_duration_secs = self
            .game_data_cache
            .snapshot()
            .game_rules
            .data
            .agent_state
            .tick
            .real_seconds_per_tick as i64;
        let recent_actions_map = {
            // 只回溯 2 个 tick 的动作，控制 DB 负载
            let since_tick = tick_id - tick_duration_secs * 2;
            match crate::db::get_recent_actions_batch(
                &self.db_pool,
                &co_located_ids,
                since_tick,
                3, // 每人最多 3 条
            )
            .await
            {
                Ok(map) => map,
                Err(e) => {
                    warn!("reactive WorldState: 加载 recent_actions 失败: {}", e);
                    HashMap::new()
                }
            }
        };

        // 4. 为每个同位置 Agent 构建个性化 WorldState 并发送
        for state in &co_located {
            let target_id = state.agent_id;
            let inventory = inventories
                .get(&target_id)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            let config = ItemRegistry::get(&item.item_id);
                            crate::models::InventoryItem {
                                // 协议层携带物品 uuid（v5 派生），动作边界反解
                                item_id: crate::items::item_uuid(&item.item_id).to_string(),
                                name: config
                                    .as_ref()
                                    .map(|c| c.name.clone())
                                    .unwrap_or_else(|| item.item_id.clone()),
                                quantity: item.quantity,
                                is_equipped: item.is_equipped,
                                item_type: config
                                    .as_ref()
                                    .map(|c| c.item_type.clone())
                                    .unwrap_or_default(),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let nearby = ground_items
                .get(&location)
                .map(|items| {
                    items
                        .iter()
                        .map(|gi| {
                            let config = ItemRegistry::get(&gi.item_id);
                            cyber_jianghu_protocol::SceneItem {
                                item_id: crate::items::item_uuid(&gi.item_id).to_string(),
                                name: config
                                    .as_ref()
                                    .map(|c| c.name.clone())
                                    .unwrap_or_else(|| gi.item_id.clone()),
                                quantity: gi.quantity,
                                item_type: config
                                    .as_ref()
                                    .map(|c| c.item_type.clone())
                                    .unwrap_or_default(),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let gd = self.game_data_cache.snapshot();
            let loc = self.game_data_cache.location_snapshot();
            let recipe_ids = crate::db::get_known_recipe_ids(&self.db_pool, target_id)
                .await
                .unwrap_or_default();
            let recipe_details = super::super::broadcaster::build_recipe_details(&recipe_ids);
            let world_state = super::super::broadcaster::build_reactive_world_state(
                state,
                &co_located,
                tick_id,
                &inventory,
                &nearby,
                &agent_names,
                &online_ids,
                &gd,
                &loc,
                &recent_actions_map,
                events.clone(),
                recipe_details,
            );

            if let Err(e) = super::send_to_agent(
                target_id,
                &cyber_jianghu_protocol::ServerMessage::WorldState { data: world_state },
                &self.connection_manager,
                &self.agent_to_device_map,
            )
            .await
            {
                debug!(
                    "reactive WorldState 发送失败: agent={}, error={}",
                    target_id, e
                );
            }
        }

        debug!(
            "reactive WorldState: location={}, 推送 {} 个 Agent",
            location,
            co_located.len()
        );
    }

    /// 广播事件给指定 Agent
    pub(super) async fn broadcast_event(
        &self,
        target_id: uuid::Uuid,
        event: WorldEvent,
    ) -> Result<()> {
        let msg = cyber_jianghu_protocol::ServerMessage::ImmediateEvent {
            event_id: uuid::Uuid::new_v4(),
            event,
        };
        super::send_to_agent(
            target_id,
            &msg,
            &self.connection_manager,
            &self.agent_to_device_map,
        )
        .await
        .map_err(|e| anyhow::anyhow!("广播失败: {}", e))
    }

    /// 处理死亡通知：物品掉落 → DB 状态更新 → DashMap 清理 → WS 断连 → 广播
    pub(super) async fn handle_deaths(
        &self,
        notifications: Vec<decay::DeathNotification>,
        tick_id: i64,
    ) {
        for notif in &notifications {
            let agent_id = notif.agent_id;
            let location = &notif.location;

            // 死者姓名（DashMap 移除前捕获）。目击事件必须具名 —— 涌现行为
            // （哀悼/记仇/避讳）依赖目击者记住"谁"死了，而非"有人"死了。
            let deceased_name: Option<String> = self
                .state_cache
                .get(&agent_id)
                .map(|s| s.value().name.clone())
                .filter(|n| !n.is_empty());

            // 死亡归因日志 + 元数据构建（DashMap 移除前完成）
            let death_metadata = if let Some(state) = self.state_cache.get(&agent_id) {
                let attrs = &state.value().status;
                let hp = attrs.get("hp").unwrap_or(-1);
                let satiation = attrs.get("satiation").unwrap_or(-1);
                let hydration = attrs.get("hydration").unwrap_or(-1);
                let sanity = attrs.get("sanity").unwrap_or(-1);
                let birth_tick = state.value().birth_tick;
                let survival_ticks = birth_tick.map(|bt| tick_id - bt).unwrap_or(-1);
                info!(
                    "[death] agent={} cause={} tick={} hp={} satiation={} hydration={} sanity={} survival_ticks={}",
                    agent_id,
                    notif.cause,
                    tick_id,
                    hp,
                    satiation,
                    hydration,
                    sanity,
                    survival_ticks
                );
                Some(serde_json::json!({
                    "attributes": {
                        "hp": hp,
                        "satiation": satiation,
                        "hydration": hydration,
                        "sanity": sanity,
                    },
                    "birth_tick": birth_tick,
                    "survival_ticks": survival_ticks,
                    "death_tick": tick_id,
                    "cause": notif.cause,
                }))
            } else {
                None
            };

            // 0. （已移除）跨 Agent 传承 Layer 2 聚合教训：服务器不再聚合/广播
            //    死亡知识——传播为纯涌现：目击 → 记忆 → 目击者自主"说" → 扩散。
            //    死因/存活统计仍由 death_metadata 日志与 reward 结算保留（引擎侧）。

            // 1. 开启事务：物品掉落 + 标记死亡
            let mut tx = match self.db_pool.begin().await {
                Ok(tx) => tx,
                Err(e) => {
                    warn!("处理死亡开启事务失败: agent={}, error={}", agent_id, e);
                    continue;
                }
            };

            match crate::inventory::InventoryManager::clear_inventory(&mut tx, agent_id).await {
                Ok(items) => {
                    for item in items {
                        if let Err(e) = crate::db::add_ground_item(
                            &mut tx,
                            location,
                            &item.item_id,
                            item.quantity,
                            Some(agent_id),
                        )
                        .await
                        {
                            warn!(
                                "死亡掉落物品失败: agent={}, item={}, error={}",
                                agent_id, item.item_id, e
                            );
                        }
                    }
                }
                Err(e) => warn!("清空死亡Agent {} 背包失败: {}", agent_id, e),
            }

            // 2. DB: 标记 Agent 为 dead（不设 retired_at，死亡 ≠ 归隐）
            if let Err(e) = sqlx::query(
                "UPDATE agents SET status = 'dead' WHERE agent_id = $1 AND status = 'active'",
            )
            .bind(agent_id)
            .execute(&mut *tx)
            .await
            {
                warn!("标记 Agent {} 为 dead 失败: {}", agent_id, e);
            }

            if let Err(e) = tx.commit().await {
                warn!("提交死亡处理事务失败: agent={}, error={}", agent_id, e);
                continue;
            }

            // 3. DashMap: 移除死亡 Agent
            self.state_cache.remove(&agent_id);

            // 4. 广播死亡事件给同位置 Agent（不含死者自身）
            {
                // 目击者筛选 + 事件构造（纯函数，不变量由 decay 模块单测覆盖）
                let co_states: Vec<AgentState> = self
                    .state_cache
                    .iter()
                    .filter(|r| r.value().node_id == *location)
                    .map(|r| r.value().clone())
                    .collect();
                let same_location_agents = decay::select_witnesses(&co_states, location, agent_id);

                let event = decay::build_witness_death_event(notif, deceased_name.as_deref());

                for target_id in same_location_agents {
                    if let Err(e) = self.broadcast_event(target_id, event.clone()).await {
                        warn!("死亡事件广播失败: target={}, error={}", target_id, e);
                    }
                }

                // 4.5 reactive WorldState 推送：把具名死亡事件带入同位置 Agent 的
                // events_log —— 记忆 + 特质演化的规范通道。ImmediateEvent 只保证
                // "看见"（triage 紧急提示），本推送使目击事件进入"记住并受其影响"
                // 通道（WitnessedDeath → 恐惧/沮丧 特质变化 + 权重 1.0 情节记忆）。
                // best-effort：WorldState 走 watch latest-wins 通道，目击者未及
                // 消费的事件可能被后续广播覆写。
                self.send_reactive_world_state(location, tick_id, vec![event.clone()])
                    .await;
            }

            // 5. 发送 AgentDied + (可选) WebSocket Close
            let rebirth_delay = self
                .game_data_cache
                .snapshot()
                .game_rules
                .data
                .agent_state
                .survival
                .rebirth
                .delay_ticks;
            let ctx = DeathNotificationContext {
                connection_manager: &self.connection_manager,
                agent_to_device_map: &self.agent_to_device_map,
                rebirth_delay_ticks: rebirth_delay,
                death_metadata,
            };
            if let Err(e) = send_agent_died_notification(
                agent_id,
                notif.cause.clone(),
                notif.description.clone(),
                notif.location.clone(),
                notif.tick_id,
                notif.died_at,
                &ctx,
            )
            .await
            {
                warn!("AgentDied 通知发送失败: agent={}, error={}", agent_id, e);
            }

            // 6. 无论是否自动重生，都立即移除旧 agent 的广播映射。
            // 自动重生继续依赖同一 device_id 的 WebSocket 存活，但旧角色不能再接收后续广播。
            self.agent_to_device_map.write().await.remove(&agent_id);

            info!(
                "Agent {} 已死亡处理完成: cause={}, location={}",
                agent_id, notif.cause, location
            );
        }
    }
}
