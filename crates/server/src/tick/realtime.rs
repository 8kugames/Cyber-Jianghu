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
use crate::game_data::registry::{ActionRegistry, ItemRegistry};
use crate::game_data::types::actions::Transmission;
use crate::governance::ServerGovernanceMapper;
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

    async fn close_session_if_whisper(
        &self,
        action_type: &str,
        intent: &cyber_jianghu_protocol::Intent,
    ) {
        let is_session = ActionRegistry::get(action_type)
            .map(|c| c.transmission == Transmission::Session)
            .unwrap_or(false);
        if is_session && let Some(ref session_id) = intent.session_id {
            self.dialogue_manager.close_session(session_id).await;
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
            agent_id,
            action_type,
            intent_id,
            intent.subsequent_intents.len()
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

        // 2.1 P1 fix (#50): 校验 agents.status='active'
        // DashMap 可能残留 retired/dead 的历史 agent（#49 启动加载已修，
        // 但运行期 rebirth 后旧 agent_id 仍可能在 DashMap 中残留），
        // 此处对 DB 二次校验，拒绝非 active 的 intent。
        let agent_db_status: Option<String> =
            sqlx::query_scalar("SELECT status FROM agents WHERE agent_id = $1")
                .bind(agent_id)
                .fetch_optional(&self.db_pool)
                .await
                .context(format!("查询 Agent {} 状态失败", agent_id))?;

        match agent_db_status.as_deref() {
            Some("active") => {}
            other => {
                let (code, msg) = match other {
                    Some(s) => (
                        "agent_not_active",
                        format!("Agent 状态为 {}，不接受 intent", s),
                    ),
                    None => ("agent_not_found", "Agent 不存在".to_string()),
                };
                warn!(
                    "Intent 被拒绝: agent={} status={:?}（非 active），DashMap 残留",
                    agent_id, agent_db_status
                );
                self.send_error_to_agent(agent_id, intent_id, code, &msg, agent_state.tick_id)
                    .await;
                self.state_cache.remove(&agent_id);
                return Ok(());
            }
        }

        // 3. 收集 DashMap 快照（供跨 Agent 校验）
        let all_states: Vec<AgentState> =
            self.state_cache.iter().map(|r| r.value().clone()).collect();

        // 4. 通过 StateProcessor 执行
        let tick_id = agent_state.tick_id; // 使用当前 tick_id
        let pre_node_id = agent_state.node_id.clone();
        let pre_skills = agent_state.skills.clone();
        let result = match self
            .state_processor
            .process_single_intent(tick_id, agent_state, &intent, &all_states, 0)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // 执行失败 → 反馈给 Agent + 释放 whisper session
                self.send_error_to_agent(
                    agent_id,
                    intent_id,
                    "execution_failed",
                    &format!("Intent 执行失败: {}", e),
                    tick_id,
                )
                .await;
                self.close_session_if_whisper(&action_type, &intent).await;
                return Err(e.context(format!("Intent 执行失败: agent={}", agent_id)));
            }
        };

        // DEBUG: Intent 执行后状态检查
        {
            let post_node_id = &result.updated_state.node_id;
            let post_tick_id = result.updated_state.tick_id;
            if pre_node_id != *post_node_id || action_type == "移动" {
                info!(
                    "[DEBUG-MOVE] agent={}, action={}, tick={}, pre_node={}, post_node={}, post_tick={}",
                    agent_id, action_type, tick_id, pre_node_id, post_node_id, post_tick_id
                );
            }
        }

        // 5. 持久化到 DB（await 确认）
        if let Err(e) = crate::db::upsert_agent_state(&self.db_pool, &result.updated_state).await {
            // persist 失败 → DashMap 不更新 → 反馈失败给 Agent + 释放 whisper session
            self.send_error_to_agent(
                agent_id,
                intent_id,
                "persist_failed",
                "状态持久化失败，Intent 未生效",
                tick_id,
            )
            .await;
            self.close_session_if_whisper(&action_type, &intent).await;
            return Err(e).context(format!("Agent {} 状态持久化失败", agent_id));
        }

        // 6. 更新 DashMap（persist 成功后）
        self.state_cache
            .insert(agent_id, result.updated_state.clone());

        // 6.5 技能习得推送：检测新增技能，推送 SkillContent 给 Agent
        let new_skills: Vec<String> = result
            .updated_state
            .skills
            .iter()
            .filter(|s| !pre_skills.contains(s))
            .cloned()
            .collect();

        if !new_skills.is_empty() {
            let all_skills = crate::game_data::registry::SkillRegistry::all_with_id();
            let skill_contents: Vec<cyber_jianghu_protocol::types::SkillContent> = all_skills
                .into_iter()
                .filter(|s| new_skills.contains(&s.skill_id))
                .map(|s| cyber_jianghu_protocol::types::SkillContent {
                    skill_id: s.skill_id,
                    name: s.definition.name,
                    body: s.definition.content,
                })
                .collect();

            if !skill_contents.is_empty() {
                let config_update = cyber_jianghu_protocol::ServerMessage::ConfigUpdate {
                    config_type: "skills".to_string(),
                    update_type: "incremental".to_string(),
                    version: "1.0.0".to_string(),
                    content: serde_json::to_value(&skill_contents).unwrap_or_default(),
                    content_hash: None,
                    updated_items: skill_contents.iter().map(|s| s.skill_id.clone()).collect(),
                    removed_items: vec![],
                };

                if let Err(e) = super::send_to_agent(
                    agent_id,
                    &config_update,
                    &self.connection_manager,
                    &self.agent_to_device_map,
                )
                .await
                {
                    warn!(
                        "Skill ConfigUpdate 推送失败: agent={}, error={}",
                        agent_id, e
                    );
                } else {
                    info!(
                        "Skill ConfigUpdate 已推送: agent={}, skills={:?}",
                        agent_id,
                        skill_contents
                            .iter()
                            .map(|s| &s.skill_id)
                            .collect::<Vec<_>>()
                    );
                }
            }
        }

        // 7. 广播 ExecutionResult 给提交 Agent
        self.send_execution_result(
            agent_id,
            intent_id,
            tick_id,
            true,
            None,
            Some(action_type.clone()),
            None,
        )
        .await;

        // 8. 交互驱动即时推送 WorldState（提交 Agent + 同位置 Agent）
        let events: Vec<WorldEvent> = result.events.iter().map(|(_, e)| e.clone()).collect();
        self.send_reactive_world_state(agent_id, tick_id, events)
            .await;

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
        for (seq, subsequent) in intent.subsequent_intents.iter().enumerate() {
            let pipe_seq = (seq + 1) as i32;
            debug!(
                "处理 subsequent Intent: agent={}, action={}, pipe_seq={}",
                agent_id, subsequent.action_type, pipe_seq
            );
            if let Err(e) = self
                .process_single_subsequent(subsequent, agent_id, tick_id, pipe_seq)
                .await
            {
                warn!(
                    "Subsequent intent 失败，中断 pipeline: agent={}, action={}, pipe_seq={}, error={}",
                    agent_id, subsequent.action_type, pipe_seq, e
                );
                break;
            }
        }

        // 11. Action 致死善后：state_processor 可能在 StateChange 处理中将
        //     is_alive 翻转为 false（HP 归零、stamina 归零、显式 AgentDied 等）。
        //     历史路径分裂：
        //       - decay 自然死亡（satiation/hydration/sanity 衰减）→ decay 模块
        //         生成 DeathNotification → handle_deaths 善后（status='dead' 回写、
        //         物品掉落、DashMap 移除、AgentDied WS、同位置广播）
        //       - action 致死 → 仅设 is_alive=false，缺 status='dead' 回写，
        //         导致 auto_rebirth SQL WHERE status='dead' 0 行命中，agent 永久卡死。
        //     修复：检测 is_alive=false 时复用 handle_deaths 完成统一善后。
        //     step 6 的 state_cache.insert 已写入 is_alive=false 的最终态，
        //     handle_deaths 内部仍能从中读取死亡元数据（hp/sat/hyd/sanity/birth_tick）。
        if !result.updated_state.is_alive {
            let death_notif = decay::DeathNotification::new(
                agent_id,
                "action".to_string(),
                format!("Action 致死: {}", action_type),
                result.updated_state.node_id.clone(),
                tick_id,
            );
            info!(
                "[death] action 致死触发善后: agent={}, action={}, tick={}, node={}",
                agent_id, action_type, tick_id, result.updated_state.node_id
            );
            self.handle_deaths(vec![death_notif], tick_id).await;
        }

        // 12. Whisper 执行后立即释放 session（避免同 tick 内 AlreadyInDialogue）
        self.close_session_if_whisper(&action_type, &intent).await;

        Ok(())
    }

    /// 处理 subsequent intent（从 pipeline 中的后续动作）
    async fn process_single_subsequent(
        &self,
        intent: &cyber_jianghu_protocol::Intent,
        agent_id: uuid::Uuid,
        tick_id: i64,
        pipe_seq: i32,
    ) -> Result<()> {
        // 从 DashMap 读取最新状态（前一个 intent 已更新）
        let agent_state = self
            .state_cache
            .get(&agent_id)
            .map(|r| r.value().clone())
            .ok_or_else(|| anyhow::anyhow!("Agent {} 不在缓存中", agent_id))?;

        if !agent_state.is_alive {
            self.send_error_to_agent(
                agent_id,
                intent.intent_id,
                "agent_dead",
                "Agent 已死亡",
                tick_id,
            )
            .await;
            return Err(anyhow::anyhow!("Agent 已死亡"));
        }

        let all_states: Vec<AgentState> =
            self.state_cache.iter().map(|r| r.value().clone()).collect();

        let result = self
            .state_processor
            .process_single_intent(tick_id, agent_state, intent, &all_states, pipe_seq)
            .await;

        let (updated_state, event_tuples) = match result {
            Ok(r) => (r.updated_state, r.events),
            Err(e) => {
                // 执行失败 → 发 failure notification + 清理 whisper session
                self.send_error_to_agent(
                    agent_id,
                    intent.intent_id,
                    "execution_failed",
                    &format!("Intent 执行失败: {}", e),
                    tick_id,
                )
                .await;
                self.close_session_if_whisper(intent.action_type.as_ref(), intent)
                    .await;
                return Err(e).context("Subsequent intent 执行失败");
            }
        };

        // 持久化
        if let Err(e) = crate::db::upsert_agent_state(&self.db_pool, &updated_state).await {
            // persist 失败 → 发 failure notification + 清理 whisper session（不更新 DashMap）
            self.send_error_to_agent(
                agent_id,
                intent.intent_id,
                "persist_failed",
                "状态持久化失败，Intent 未生效",
                tick_id,
            )
            .await;
            self.close_session_if_whisper(intent.action_type.as_ref(), intent)
                .await;
            return Err(e).context("Subsequent intent 持久化失败");
        }

        // persist 成功后更新 DashMap
        self.state_cache.insert(agent_id, updated_state.clone());

        // 发成功通知
        self.send_execution_result(
            agent_id,
            intent.intent_id,
            tick_id,
            true,
            None,
            Some(intent.action_type.to_string()),
            None,
        )
        .await;

        // 提取纯 WorldEvent Vec 用于 reactive push，保留元组用于 broadcast
        let events: Vec<WorldEvent> = event_tuples.iter().map(|(_, e)| e.clone()).collect();
        self.send_reactive_world_state(agent_id, tick_id, events)
            .await;

        for (target_id, event) in &event_tuples {
            if let Err(e) = self.broadcast_event(*target_id, event.clone()).await {
                warn!("事件广播失败: target={}, error={}", target_id, e);
            }
        }

        // P0 修复（subsequent 路径）：与 process_single_intent step 11 对齐。
        // 历史 bug：action 致死（HP 归零、stamina 归零等）只在主 intent 末尾检测，
        // subsequent intent（pipe_seq > 0）中的死亡漏检，导致 status='active' 卡死、
        // auto_rebirth 永久拒绝。复用 handle_deaths 完成统一善后。
        if !updated_state.is_alive {
            let death_notif = decay::DeathNotification::new(
                agent_id,
                "action".to_string(),
                format!(
                    "Action 致死 (subsequent pipe_seq={}): {}",
                    pipe_seq, intent.action_type
                ),
                updated_state.node_id.clone(),
                tick_id,
            );
            info!(
                "[death] action 致死触发善后 (subsequent): agent={}, action={}, pipe_seq={}, tick={}, node={}",
                agent_id, intent.action_type, pipe_seq, tick_id, updated_state.node_id
            );
            self.handle_deaths(vec![death_notif], tick_id).await;
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

        // DEBUG: 打印 DashMap 中所有 agent 的 node_id
        for s in &states {
            if s.node_id != "龙门大堂" {
                info!(
                    "[DEBUG-TICK] Tick {}: agent={} node={} alive={}",
                    tick_id, s.agent_id, s.node_id, s.is_alive
                );
            }
        }

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
                    warn!(
                        "Tick {}: ghost agent {} 持久化失败: {}",
                        tick_id, state.agent_id, e
                    );
                    ghost_ids.push(state.agent_id);
                }
            }
        }

        // 4. 关闭 ghost agent 的 WebSocket 连接，然后从 DashMap 移除
        //    否则客户端连接存活但状态已清，造成"幽灵黑洞"
        for agent_id in &ghost_ids {
            // 4a. 发送错误通知，让客户端知悉需要断连重连
            let device_id = {
                let map = self.agent_to_device_map.read().await;
                map.get(agent_id).copied()
            };
            if let Some(device_id) = device_id {
                let error_msg = cyber_jianghu_protocol::ServerMessage::Error {
                    code: cyber_jianghu_protocol::ERROR_CODE_AGENT_DEAD.into(),
                    message: format!("状态持久化失败 (agent_id={})，请断连后重新连接", agent_id),
                    current_tick_id: Some(tick_id),
                };
                if let Ok(json) = serde_json::to_string(&error_msg) {
                    let mut connections = self.connection_manager.write().await;
                    if let Some(conn) = connections.get_mut(&device_id) {
                        let _ = conn
                            .send(axum::extract::ws::Message::Text(json.into()))
                            .await;
                    }
                }
                // 4b. 强制关闭 WebSocket 连接
                {
                    let mut connections = self.connection_manager.write().await;
                    connections.remove(&device_id);
                }
                // 4c. 清除 agent→device 映射
                {
                    let mut map = self.agent_to_device_map.write().await;
                    map.remove(agent_id);
                }
                info!(
                    "Tick {}: ghost agent {} WebSocket 已强制关闭 (device={})",
                    tick_id, agent_id, device_id
                );
            }
            // 4d. 从 DashMap 移除
            self.state_cache.remove(agent_id);
            info!(
                "Tick {}: ghost agent {} 已从 DashMap 清除",
                tick_id, agent_id
            );
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
    #[allow(clippy::too_many_arguments)]
    async fn send_execution_result(
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
    async fn send_error_to_agent(
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
    /// Intent 执行后，为提交 Agent 及同位置在线 Agent 构建并发送最新 WorldState。
    /// 确保 Agent 在下一次认知决策前拥有最新的世界状态。
    async fn send_reactive_world_state(
        &self,
        agent_id: Uuid,
        tick_id: i64,
        events: Vec<WorldEvent>,
    ) {
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

            let gd = self.game_data_cache.snapshot();
            let loc = self.game_data_cache.location_snapshot();
            let recipe_ids = crate::db::get_known_recipe_ids(&self.db_pool, target_id)
                .await
                .unwrap_or_default();
            let recipe_details = super::broadcaster::build_recipe_details(&recipe_ids);
            let world_state = super::broadcaster::build_reactive_world_state(
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

            // 0. 跨 Agent 传承 Layer 2: 记录教训
            {
                let survival = death_metadata
                    .as_ref()
                    .and_then(|m| m.get("survival_ticks"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(-1);
                let gd = self.game_data_cache.snapshot();
                let lesson_cfg = gd.game_rules.data.lesson.as_ref();
                let threshold = lesson_cfg.map(|c| c.threshold).unwrap_or(
                    crate::game_data::types::unified_config::LessonConfig::DEFAULT_THRESHOLD,
                );
                let cause_map = lesson_cfg
                    .map(|c| c.cause_advice_map.clone())
                    .unwrap_or_default();
                super::lessons::record_death_lesson(
                    &self.db_pool,
                    &notif.cause,
                    survival,
                    tick_id,
                    threshold,
                    &cause_map,
                )
                .await;
            }

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

            // 2. DB: 标记 Agent 为 dead（不设 retired_at，死亡 ≠ 归隐）
            if let Err(e) = sqlx::query(
                "UPDATE agents SET status = 'dead' WHERE agent_id = $1 AND status = 'active'",
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

            // 6. 自动重生时不断连，保留 agent_to_device_map 映射
            if rebirth_delay <= 0 {
                self.agent_to_device_map.write().await.remove(&agent_id);
            }

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
