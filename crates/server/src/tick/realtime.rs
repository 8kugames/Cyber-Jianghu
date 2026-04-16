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

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::db::DbPool;
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
}

impl IntentWorker {
    pub fn new(
        db_pool: DbPool,
        state_cache: AgentStateCache,
        state_processor: Arc<StateProcessor>,
        connection_manager: ConnectionManager,
        agent_to_device_map: AgentToDeviceMap,
    ) -> Self {
        Self {
            db_pool,
            state_cache,
            state_processor,
            connection_manager,
            agent_to_device_map,
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
            "处理 Intent: agent={}, action={}, intent={}",
            agent_id, action_type, intent_id
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
        let all_states: Vec<AgentState> = self
            .state_cache
            .iter()
            .map(|r| r.value().clone())
            .collect();

        // 4. 通过 StateProcessor 执行
        let tick_id = agent_state.tick_id; // 使用当前 tick_id
        let result = self
            .state_processor
            .process_single_intent(tick_id, agent_state, &intent, &all_states)
            .await
            .context(format!("Intent 执行失败: agent={}", agent_id))?;

        // 5. 持久化到 DB（await 确认）
        crate::db::upsert_agent_state(&self.db_pool, &result.updated_state)
            .await
            .context(format!("Agent {} 状态持久化失败", agent_id))?;

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

        // 8. 广播事件给同位置 Agent
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

        Ok(())
    }

    // ========================================================================
    // Tick 边界处理
    // ========================================================================

    /// 处理 Tick 边界：衰减 + 持久化 + 广播 WorldState
    async fn process_tick_boundary(&self, tick_id: i64) -> Result<()> {
        debug!("Tick {} 边界处理开始", tick_id);

        // 1. 从 DashMap 读取所有 Agent 状态
        let states: Vec<AgentState> = self.state_cache.iter().map(|r| r.value().clone()).collect();

        if states.is_empty() {
            debug!("Tick {}: 无存活 Agent，跳过衰减", tick_id);
            return Ok(());
        }

        // 2. 衰减
        let (updated_states, dead_agents, _decay_events, death_notifications) =
            decay::apply_decay_and_environmental_damage(tick_id, states);

        // 3. 批量持久化衰减结果
        persistence::persist_states(&self.db_pool, tick_id, &updated_states)
            .await
            .context("衰减状态持久化失败")?;

        // 4. 更新 DashMap（persist 成功后）
        for state in &updated_states {
            self.state_cache.insert(state.agent_id, state.clone());
        }

        // 5. 处理死亡
        if !dead_agents.is_empty() {
            info!("Tick {}: {} 个 Agent 死亡", tick_id, dead_agents.len());
            self.handle_deaths(death_notifications, tick_id).await;
        }

        // 6. 广播周期 WorldState
        // TODO: 调用 Broadcaster::broadcast_states() 广播给所有 Agent
        // 当前先跳过，Phase 2 完善

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

    /// 广播事件给指定 Agent
    async fn broadcast_event(&self, target_id: uuid::Uuid, event: WorldEvent) -> Result<()> {
        let msg = cyber_jianghu_protocol::ServerMessage::ImmediateEvent {
            event_id: uuid::Uuid::new_v4(),
            event,
            deadline_ms: 0,
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
                            warn!("死亡掉落物品失败: agent={}, item={}, error={}", agent_id, item.item_id, e);
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
                    description: format!(
                        "有人在 {} 亡故：{}",
                        location, notif.description
                    ),
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
