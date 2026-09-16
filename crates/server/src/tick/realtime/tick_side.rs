// ============================================================================
// tick 边界处理（process_tick_boundary：衰减/死亡/广播）
// ============================================================================

use crate::tick::decay;
use crate::tick::persistence;

use tracing::{debug, info, warn};

use super::IntentWorker;
use crate::models::AgentState;
use anyhow::Result;

impl IntentWorker {
    /// 处理 Tick 边界：衰减 + 持久化 + 广播 WorldState
    pub(super) async fn process_tick_boundary(&self, tick_id: i64) -> Result<()> {
        debug!("Tick {} 边界处理开始", tick_id);

        // tick_logs 写入（修复死代码债：让 tick 完成率/崩溃数可测）
        // best-effort：失败只 warn 不阻断 tick 主流程。
        // 若本函数中途 ? 传播错误，tick_log 保持 Running 状态（自然标记未完成 tick）。
        let mut tick_log = crate::models::tick::TickLog::new(tick_id);
        if let Err(e) = crate::db::create_tick_log(&self.db_pool, &tick_log).await {
            warn!("Tick {} tick_logs 写入失败（不阻断）: {}", tick_id, e);
        }

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
        // 休息判定：本 tick 或上一 tick 内成功执行过 intent 的 Agent 视为行动中，
        // 其余（idle-skip / 离线 / 思考间隙）视为休息 tick，门控恢复生效。
        // tick_id 是秒级时间戳（scheduler::calculate_tick_id_from_time），
        // 相邻 tick 边界相差 real_seconds_per_tick 秒，窗口必须按秒换算。
        let tick_window_secs = crate::game_data::registry_or_error()
            .map(|cache| {
                cache
                    .get()
                    .game_rules
                    .data
                    .agent_state
                    .tick
                    .real_seconds_per_tick as i64
            })
            .ok()
            .filter(|v| *v > 0)
            .unwrap_or(60);
        let acted_recently: std::collections::HashSet<uuid::Uuid> = self
            .last_intent_ticks
            .iter()
            .filter(|entry| tick_id - entry.value() <= tick_window_secs)
            .map(|entry| *entry.key())
            .collect();
        let (mut updated_states, dead_agents, _decay_events, death_notifications) =
            decay::apply_decay_and_environmental_damage(tick_id, states, &acted_recently);

        // 2.1 更新 tick_id 到当前 tick（衰减不更新 tick_id，需显式设置）
        for state in &mut updated_states {
            state.tick_id = tick_id;
            state.state_version = 0;
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
            // 生存 Reward 一生结算（旁路，失败只 warn 不阻断 tick；幂等：按 agent_id 覆盖）
            for dead_id in &dead_agents {
                if let Err(e) = crate::reward::settle_lifetime(&self.db_pool, *dead_id).await {
                    warn!(
                        "[reward] 一生结算失败 (agent={}, tick={}): {}",
                        dead_id, tick_id, e
                    );
                }
            }
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

        // tick_logs 更新为完成（best-effort）
        tick_log.complete(updated_states.len() as i32, 0);
        if let Err(e) = crate::db::update_tick_log(&self.db_pool, &tick_log).await {
            warn!("Tick {} tick_logs 更新失败（不阻断）: {}", tick_id, e);
        }

        Ok(())
    }

    // ========================================================================
    // 广播辅助方法
    // ========================================================================
}
