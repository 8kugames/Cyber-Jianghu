// ============================================================================
// 实时 Intent 处理引擎
// ============================================================================
//
// IntentWorker 是单消费者事件循环，顺序处理两类消息：
// - Intent: Agent 提交的意图，立即验证+执行+持久化+广播结果
// - TickBoundary: Tick 周期信号，执行衰减+批量持久化+广播 WorldState
//
// 设计原则：
// - 单消费者消除所有竞态（DashMap 不存在并发写入冲突）
// - write-through: persist 到 DB 确认后才更新 DashMap
// - 非阻塞: handler.rs 用 try_send，队列满时返回错误而非 block

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::db::DbPool;
use crate::dialogue::DialogueManager;
use crate::game_data::GameDataCache;
use crate::game_data::registry::ItemRegistry;
use crate::models::{AgentState, WorldEvent, WorldEventType};
use crate::state::AgentStateCache;
use crate::tick::decay;
use crate::tick::persistence;
use crate::websocket::{
    AgentToDeviceMap, ConnectionManager, DeathNotificationContext, send_agent_died_notification,
};

use super::processor::StateProcessor;

// ============================================================================
// Worker Message 枚举
// ============================================================================

/// IntentWorker 消息类型
///
/// 单一 channel 传递两种消息，保证顺序处理、零竞态。
pub enum WorkerMessage {
    /// Agent 提交的意图（实时处理）
    Intent {
        intent: Box<cyber_jianghu_protocol::Intent>,
    },
    /// Tick 周期边界信号（衰减 + 广播）
    TickBoundary { tick_id: i64 },
}

// ============================================================================
// IntentWorker
// ============================================================================

/// 实时 Intent 处理引擎
pub struct IntentWorker {
    /// 数据库连接池
    db_pool: DbPool,
    /// Agent 状态内存缓存
    state_cache: AgentStateCache,
    /// 状态处理器（验证 + 执行 + 状态变更）
    state_processor: Arc<StateProcessor>,
    /// WebSocket 连接管理器（广播用）
    connection_manager: ConnectionManager,
    /// agent_id → device_id 映射（广播用）
    agent_to_device_map: AgentToDeviceMap,
    /// 对话管理器（whisper session 生命周期管理）
    dialogue_manager: Arc<DialogueManager>,
    /// 游戏数据缓存（构建 WorldState 用）
    game_data_cache: Arc<GameDataCache>,
}

impl IntentWorker {
    pub fn new(
        db_pool: DbPool,
        state_cache: AgentStateCache,
        state_processor: Arc<StateProcessor>,
        connection_manager: ConnectionManager,
        agent_to_device_map: AgentToDeviceMap,
        dialogue_manager: Arc<DialogueManager>,
        game_data_cache: Arc<GameDataCache>,
    ) -> Self {
        Self {
            db_pool,
            state_cache,
            state_processor,
            connection_manager,
            agent_to_device_map,
            dialogue_manager,
            game_data_cache,
        }
    }

    /// 启动 Worker 事件循环
    ///
    /// 消费 MPSC channel 直到发送端关闭（server shutdown）。
    pub async fn run(self, mut rx: mpsc::Receiver<WorkerMessage>) {
        info!("IntentWorker 启动");

        while let Some(msg) = rx.recv().await {
            match msg {
                WorkerMessage::Intent { intent } => {
                    if let Err(e) = self.process_intent(*intent).await {
                        // 错误已在 process_intent 内部记录
                        warn!("Intent 处理失败: {}", e);
                    }
                }
                WorkerMessage::TickBoundary { tick_id } => {
                    if let Err(e) = self.process_tick_boundary(tick_id).await {
                        error!("Tick {} 边界处理失败: {}", tick_id, e);
                    }
                }
            }
        }

        info!("IntentWorker 停止（channel 关闭）");
    }

    // ========================================================================
    // Intent 处理
    // ========================================================================

    /// 处理单条 Intent
    async fn process_intent(&self, intent: cyber_jianghu_protocol::Intent) -> Result<()> {
        let agent_id = intent.agent_id;
        let action_type = intent.action_type.to_string();
        let intent_id = intent.intent_id;

        debug!(
            "处理 Intent: agent={}, action={}, intent={}, subsequent={}",
            agent_id, action_type, intent_id, intent.subsequent_intents.len()
        );

        // 1. 从 DashMap 读取 Agent 状态
        let agent_state = self
            .state_cache
            .get(&agent_id)
            .map(|r| r.value().clone())
            .ok_or_else(|| anyhow::anyhow!("Agent {} 不在缓存中", agent_id))?;

        // 2. 校验存活
        if !agent_state.is_alive {
            self.send_error_to_agent(
                agent_id,
                intent_id,
                "agent_dead",
                "Agent 已死亡",
                agent_state.tick_id,
            )
            .await;
            return Ok(());
        }

        // 3. 收集 DashMap 快照（供跨 Agent 校验）
        let all_states: Vec<AgentState> =
            self.state_cache.iter().map(|r| r.value().clone()).collect();

        // 4. 通过 StateProcessor 执行
        let tick_id = agent_state.tick_id; // 使用当前 tick_id
        let result = match self
            .state_processor
            .process_single_intent(tick_id, agent_state, &intent, &all_states)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // 执行失败 → 反馈给 Agent
                self.send_error_to_agent(
                    agent_id,
                    intent_id,
                    "execution_failed",
                    &format!("Intent 执行失败: {}", e),
                    tick_id,
                )
                .await;
                return Err(e.context(format!("Intent 执行失败: agent={}", agent_id)));
            }
        };

        // 5. 持久化到 DB（await 确认）
        if let Err(e) = crate::db::upsert_agent_state(&self.db_pool, &result.updated_state).await {
            // persist 失败 → DashMap 不更新 → 反馈失败给 Agent
            self.send_error_to_agent(
                agent_id,
                intent_id,
                "persist_failed",
                "状态持久化失败，Intent 未生效",
                tick_id,
            )
            .await;
            return Err(e).context(format!("Agent {} 状态持久化失败", agent_id));
        }

        // 6. 更新 DashMap（persist 成功后）
        self.state_cache
            .insert(agent_id, result.updated_state.clone());

        // 7. 广播 ExecutionResult 给提交 Agent
        self.send_execution_result(
            agent_id,
            intent_id,
            tick_id,
            true,
            None,
            Some(action_type.clone()),
        )
        .await;

        // 8. 交互驱动即时推送 WorldState（提交 Agent + 同位置 Agent）
        self.send_reactive_world_state(agent_id, tick_id).await;

        // 9. 广播事件给同位置 Agent
        for (target_id, event) in &result.events {
            if let Err(e) = self.broadcast_event(*target_id, event.clone()).await {
                warn!("事件广播失败: target={}, error={}", target_id, e);
            }
        }

        debug!(
            "Intent 处理完成: agent={}, action={}, events={}",
            agent_id,
            action_type,
            result.events.len()
        );

        // 10. 处理 subsequent_intents（按顺序，任一失败则中断）
        for subsequent in &intent.subsequent_intents {
            debug!(
                "处理 subsequent Intent: agent={}, action={}",
                agent_id, subsequent.action_type
            );
            if let Err(e) = self.process_single_subsequent(subsequent, agent_id, tick_id).await {
                warn!(
                    "Subsequent intent 失败，中断 pipeline: agent={}, action={}, error={}",
                    agent_id, subsequent.action_type, e
                );
                break;
            }
        }

        Ok(())
    }

    /// 处理 subsequent intent（从 pipeline 中的后续动作）
    async fn process_single_subsequent(
        &self,
        intent: &cyber_jianghu_protocol::Intent,
        agent_id: uuid::Uuid,
        tick_id: i64,
    ) -> Result<()> {
        // 从 DashMap 读取最新状态（前一个 intent 已更新）
        let agent_state = self
            .state_cache
            .get(&agent_id)
            .map(|r| r.value().clone())
            .ok_or_else(|| anyhow::anyhow!("Agent {} 不在缓存中", agent_id))?;

        if !agent_state.is_alive {
            return Err(anyhow::anyhow!("Agent 已死亡"));
        }

        let all_states: Vec<AgentState> =
            self.state_cache.iter().map(|r| r.value().clone()).collect();

        let result = self
            .state_processor
            .process_single_intent(tick_id, agent_state, intent, &all_states)
            .await?;

        if let Err(e) = crate::db::upsert_agent_state(&self.db_pool, &result.updated_state).await {
            return Err(e).context("Subsequent intent 持久化失败");
        }

        self.state_cache
            .insert(agent_id, result.updated_state.clone());

        self.send_execution_result(
            agent_id,
            intent.intent_id,
            tick_id,
            true,
            None,
            Some(intent.action_type.to_string()),
        )
        .await;

        self.send_reactive_world_state(agent_id, tick_id).await;

        for (target_id, event) in &result.events {
            if let Err(e) = self.broadcast_event(*target_id, event.clone()).await {
                warn!("事件广播失败: target={}, error={}", target_id, e);
            }
        }

        Ok(())
    }

    // ========================================================================
    // Tick 边界处理
    // ========================================================================

    /// 处理 Tick 边界：衰减 + 持久化 + 广播 WorldState
    async fn process_tick_boundary(&self, tick_id: i64) -> Result<()> {
        debug!("Tick {} 边界处理开始", tick_id);

        // 1. 从 DashMap 读取所有 Agent 状态
        let mut ghost_ids: Vec<uuid::Uuid> = Vec::new();
        let states: Vec<AgentState> = self.state_cache.iter().map(|r| r.value().clone()).collect();

        if states.is_empty() {
            debug!("Tick {}: 无存活 Agent，跳过衰减", tick_id);
            return Ok(());
        }

        // 2. 衰减
        let (mut updated_states, dead_agents, _decay_events, death_notifications) =
            decay::apply_decay_and_environmental_damage(tick_id, states);

        // 2.1 更新 tick_id 到当前 tick（衰减不更新 tick_id，需显式设置）
        for state in &mut updated_states {
            state.tick_id = tick_id;
        }

        // 3. 批量持久化衰减结果（失败时回退到逐条 persist 并清除 ghost agent）
        if let Err(e) = persistence::persist_states(&self.db_pool, tick_id, &updated_states).await {
            warn!(
                "Tick {} 批量衰减持久化失败，回退到逐条 persist: {}",
                tick_id, e
            );
            for state in &updated_states {
                if let Err(e) = crate::db::upsert_agent_state(&self.db_pool, state).await {
                    // FK 约束失败 → ghost agent，从 DashMap 清除
                    warn!(
                        "Tick {}: ghost agent {} 持久化失败，从 DashMap 移除: {}",
                        tick_id, state.agent_id, e
                    );
                    ghost_ids.push(state.agent_id);
                }
            }
        }

        // 4. 更新 DashMap（persist 成功后），清除 ghost agent
        for id in &ghost_ids {
            self.state_cache.remove(id);
            info!("Tick {}: 已从 DashMap 清除 ghost agent {}", tick_id, id);
        }
        for state in &updated_states {
            if !ghost_ids.contains(&state.agent_id) {
                self.state_cache.insert(state.agent_id, state.clone());
            }
        }

        // 5. 处理死亡
        if !dead_agents.is_empty() {
            info!("Tick {}: {} 个 Agent 死亡", tick_id, dead_agents.len());
            self.handle_deaths(death_notifications, tick_id).await;
        }

        // 6. 关闭所有对话会话（防止 whisper session 泄漏）
        let closed_sessions = self.dialogue_manager.close_all_sessions().await;
        if !closed_sessions.is_empty() {
            debug!(
                "Tick {}: 关闭 {} 个对话会话",
                tick_id,
                closed_sessions.len()
            );
        }

        // 7. 周期 WorldState 广播由 TickScheduler 在发送 TickBoundary 后独立执行
        // IntentWorker 仅负责衰减+持久化+死亡处理，不重复广播

        debug!(
            "Tick {} 边界处理完成: agents={}, dead={}",
            tick_id,
            updated_states.len(),
            dead_agents.len()
        );

        Ok(())
    }

    // ========================================================================
    // 广播辅助方法
    // ========================================================================

    /// 发送 ExecutionResult 给指定 Agent
    async fn send_execution_result(
        &self,
        agent_id: uuid::Uuid,
        intent_id: uuid::Uuid,
        tick_id: i64,
        success: bool,
        error: Option<String>,
        state_change_summary: Option<String>,
    ) {
        let msg = cyber_jianghu_protocol::ServerMessage::ExecutionResult {
            tick_id,
            intent_id,
            success,
            error,
            state_change_summary,
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
    async fn send_error_to_agent(
        &self,
        agent_id: uuid::Uuid,
        intent_id: uuid::Uuid,
        _code: &str,
        message: &str,
        tick_id: i64,
    ) {
        self.send_execution_result(
            agent_id,
            intent_id,
            tick_id,
            false,
            Some(message.to_string()),
            None,
        )
        .await;
    }

    /// 交互驱动即时推送 WorldState
    ///
    /// Intent 执行后，为提交 Agent 及同位置在线 Agent 构建并发送最新 WorldState。
    /// 确保 Agent 在下一次认知决策前拥有最新的世界状态。
    async fn send_reactive_world_state(&self, agent_id: Uuid, tick_id: i64) {
        // 1. 从 DashMap 读取更新后的状态
        let updated_state = match self.state_cache.get(&agent_id) {
            Some(r) => r.value().clone(),
            None => {
                debug!("Agent {} 不在缓存中，跳过 reactive WorldState", agent_id);
                return;
            }
        };

        // 2. 收集同位置 Agent（含自身）
        let location = updated_state.node_id.clone();
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

        let inventories =
            match crate::inventory::InventoryManager::get_all_items_batch(&self.db_pool, &co_located_ids)
                .await
            {
                Ok(batch) => batch,
                Err(e) => {
                    warn!("reactive WorldState: 加载背包失败: {}", e);
                    HashMap::new()
                }
            };

        let ground_items =
            match crate::db::get_ground_items_by_nodes(&self.db_pool, std::slice::from_ref(&location)).await {
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
                                item_id: item.item_id.clone(),
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
                                item_id: gi.item_id.clone(),
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

            let world_state = super::broadcaster::build_reactive_world_state(
                state,
                &co_located,
                tick_id,
                &inventory,
                &nearby,
                &agent_names,
                &online_ids,
                &self.game_data_cache,
            );

            if let Err(e) = super::send_to_agent(
                target_id,
                &cyber_jianghu_protocol::ServerMessage::WorldState {
                    data: world_state,
                },
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
            "reactive WorldState: agent={}, location={}, 推送 {} 个 Agent",
            agent_id,
            location,
            co_located.len()
        );
    }

    /// 广播事件给指定 Agent
    async fn broadcast_event(&self, target_id: uuid::Uuid, event: WorldEvent) -> Result<()> {
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
    async fn handle_deaths(&self, notifications: Vec<decay::DeathNotification>, tick_id: i64) {
        for notif in &notifications {
            let agent_id = notif.agent_id;
            let location = &notif.location;

            // 1. 物品掉落：清空背包 → 掉落到地面
            match crate::inventory::InventoryManager::clear_inventory(&self.db_pool, agent_id).await
            {
                Ok(items) => {
                    for item in items {
                        if let Err(e) = crate::db::add_ground_item(
                            &self.db_pool,
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

            // 2. DB: 标记 Agent 为 dead
            if let Err(e) = sqlx::query(
                "UPDATE agents SET status = 'dead', retired_at = CURRENT_TIMESTAMP WHERE agent_id = $1 AND status = 'active'",
            )
            .bind(agent_id)
            .execute(&self.db_pool)
            .await
            {
                warn!("标记 Agent {} 为 dead 失败: {}", agent_id, e);
            }

            // 3. DashMap: 移除死亡 Agent
            self.state_cache.remove(&agent_id);

            // 4. 广播死亡事件给同位置 Agent（不含死者自身）
            {
                let same_location_agents: Vec<uuid::Uuid> = self
                    .state_cache
                    .iter()
                    .filter(|r| r.value().node_id == *location && r.key() != &agent_id)
                    .map(|r| *r.key())
                    .collect();

                let event = WorldEvent {
                    event_type: WorldEventType::DeathNotification,
                    tick_id,
                    description: format!("有人在 {} 亡故：{}", location, notif.description),
                    metadata: serde_json::json!({
                        "agent_id": agent_id.to_string(),
                        "cause": notif.cause,
                        "location": location,
                    }),
                };

                for target_id in same_location_agents {
                    if let Err(e) = self.broadcast_event(target_id, event.clone()).await {
                        warn!("死亡事件广播失败: target={}, error={}", target_id, e);
                    }
                }
            }

            // 5. 发送 AgentDied + WebSocket Close → 断连
            let ctx = DeathNotificationContext {
                connection_manager: &self.connection_manager,
                agent_to_device_map: &self.agent_to_device_map,
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

            // 6. 清理 agent_to_device_map（防止后续消息路由到已断连设备）
            self.agent_to_device_map.write().await.remove(&agent_id);

            info!(
                "Agent {} 已死亡处理完成: cause={}, location={}",
                agent_id, notif.cause, location
            );
        }
    }
}

// ============================================================================
// 构造辅助
// ============================================================================

/// 创建 IntentWorker channel（有界，容量 256）
pub fn create_worker_channel() -> (mpsc::Sender<WorkerMessage>, mpsc::Receiver<WorkerMessage>) {
    mpsc::channel(256)
}
