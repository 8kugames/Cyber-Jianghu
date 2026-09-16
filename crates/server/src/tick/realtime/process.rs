// ============================================================================
// Intent 处理核心（process_intent / process_single_subsequent）
// ============================================================================

use anyhow::{Context, Result};

use crate::models::{AgentState, WorldEvent};
use crate::tick::decay;

use tracing::{debug, info, warn};

use super::IntentWorker;

impl IntentWorker {
    /// 处理单条 Intent
    pub(super) async fn process_intent(
        &self,
        intent: cyber_jianghu_protocol::Intent,
    ) -> Result<()> {
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

        // 2.1 校验 agents.status='active'
        // DashMap 可能残留 retired/dead 的历史 agent（启动加载已修，
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
        let mut result = match self
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

        // 5. 持久化结果：processor 已在 tx 内完成 agent_states UPSERT 并 commit，
        //    此处仅读取返回的 state_version。`persisted_version = None` 表示
        //    tx 已 rollback（执行失败 / CAS 冲突 / commit 失败）→ DashMap 不更新。
        let persisted_version = match result.persisted_version {
            Some(version) => version,
            None => {
                // 失败分流：验证/执行失败携带具体原因，走 action_failed 反馈
                // 让 Agent 获得可纠错的拒绝理由；无原因的才是真实落库失败。
                // 修复：此前所有 rollback 一律误报"状态持久化失败"，掩盖真实
                // 拒绝原因（如"未知的动作类型"），Agent 无法自纠。
                let (code, message) = match &result.failure_reason {
                    Some(reason) => ("action_failed", reason.clone()),
                    None => (
                        "persist_failed",
                        "状态持久化失败，Intent 未生效".to_string(),
                    ),
                };
                self.send_error_to_agent(agent_id, intent_id, code, &message, tick_id)
                    .await;
                self.close_session_if_whisper(&action_type, &intent).await;

                // 词汇表自愈：未知动作拒绝时推送最新动作配置，消除部署/热更新
                // 后 Agent 词表漂移（Agent 侧 action_update 回调刷新引擎词表）
                if message.contains("未知的动作类型") {
                    self.push_fresh_actions(agent_id, tick_id).await;
                }

                if code == "persist_failed" {
                    return Err(anyhow::anyhow!(
                        "Agent {} 状态持久化失败（tx 已回滚）",
                        agent_id
                    ));
                }
                return Ok(());
            }
        };

        // 6. 更新 DashMap（persist 成功后）
        let mut persisted_state = result.updated_state.clone();
        persisted_state.state_version = persisted_version;
        self.state_cache.insert(agent_id, persisted_state.clone());
        // 记录行动 tick（休息门控恢复的反向信号：本 tick 有 intent = 非休息）
        self.last_intent_ticks.insert(agent_id, tick_id);

        // 6.15 跨 Agent 效果 write-through：目标状态回写 DashMap + 死亡善后
        // （战斗击杀在此路径触发 handle_deaths：目击广播 / AgentDied / 物品掉落）
        if !result.collateral_states.is_empty() {
            let mut dead_notifs = Vec::new();
            for (mut c_state, c_version) in result.collateral_states.drain(..) {
                let dead = !c_state.is_alive;
                c_state.state_version = c_version;
                info!(
                    "[combat] collateral write-through: agent={}, node={}, alive={}",
                    c_state.agent_id, c_state.node_id, c_state.is_alive
                );
                self.state_cache.insert(c_state.agent_id, c_state.clone());
                if dead {
                    dead_notifs.push(decay::DeathNotification::new(
                        c_state.agent_id,
                        "combat".to_string(),
                        "在战斗中被杀害".to_string(),
                        c_state.node_id.clone(),
                        tick_id,
                    ));
                }
            }
            if !dead_notifs.is_empty() {
                info!(
                    "Tick {}: {} 个 Agent 被击杀，触发善后",
                    tick_id,
                    dead_notifs.len()
                );
                self.handle_deaths(dead_notifs.clone(), tick_id).await;
                // 生存 Reward 一生结算（与 tick 边界自然死亡同构，幂等）
                for notif in &dead_notifs {
                    if let Err(e) =
                        crate::reward::settle_lifetime(&self.db_pool, notif.agent_id).await
                    {
                        warn!(
                            "[reward] 一生结算失败 (agent={}, tick={}): {}",
                            notif.agent_id, tick_id, e
                        );
                    }
                }
            }
        }

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
                    config_type: cyber_jianghu_protocol::ConfigType::Skills,
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
        self.send_reactive_world_state(&persisted_state.node_id, tick_id, events)
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
        if !persisted_state.is_alive {
            let death_notif = decay::DeathNotification::new(
                agent_id,
                "action".to_string(),
                format!("Action 致死: {}", action_type),
                persisted_state.node_id.clone(),
                tick_id,
            );
            info!(
                "[death] action 致死触发善后: agent={}, action={}, tick={}, node={}",
                agent_id, action_type, tick_id, persisted_state.node_id
            );
            self.handle_deaths(vec![death_notif], tick_id).await;
            // 生存 Reward 一生结算（与 tick 边界/tick 击杀同构，幂等）
            if let Err(e) = crate::reward::settle_lifetime(&self.db_pool, agent_id).await {
                warn!(
                    "[reward] 一生结算失败 (agent={}, tick={}): {}",
                    agent_id, tick_id, e
                );
            }
        }

        // 12. Whisper 执行后立即释放 session（避免同 tick 内 AlreadyInDialogue）
        self.close_session_if_whisper(&action_type, &intent).await;

        Ok(())
    }

    /// 处理 subsequent intent（从 pipeline 中的后续动作）
    pub(super) async fn process_single_subsequent(
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

        let (updated_state, event_tuples, persisted_version, failure_reason, mut collateral_states) =
            match result {
                Ok(r) => (
                    r.updated_state,
                    r.events,
                    r.persisted_version,
                    r.failure_reason,
                    r.collateral_states,
                ),
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

        // 持久化结果：processor 已在 tx 内完成 agent_states UPSERT 并 commit，
        // 此处仅读取返回的 state_version。`persisted_version = None` 表示
        // tx 已 rollback → 不更新 DashMap。
        let persisted_version = match persisted_version {
            Some(version) => version,
            None => {
                // 失败分流（与主 intent 路径同构）：验证/执行失败携带具体原因走
                // action_failed；仅真实落库失败才报 persist_failed。
                let (code, message) = match &failure_reason {
                    Some(reason) => ("action_failed", reason.clone()),
                    None => (
                        "persist_failed",
                        "状态持久化失败，Intent 未生效".to_string(),
                    ),
                };
                self.send_error_to_agent(agent_id, intent.intent_id, code, &message, tick_id)
                    .await;
                self.close_session_if_whisper(intent.action_type.as_ref(), intent)
                    .await;
                if message.contains("未知的动作类型") {
                    self.push_fresh_actions(agent_id, tick_id).await;
                }
                if code == "persist_failed" {
                    return Err(anyhow::anyhow!("Subsequent intent 持久化失败（tx 已回滚）"));
                }
                // 验证/执行失败：已反馈真实原因，Err 中断后续 pipeline（协议语义：失败即中断队列）
                return Err(anyhow::anyhow!("Subsequent intent 失败已反馈: {}", message));
            }
        };

        // persist 成功后更新 DashMap
        let mut persisted_state = updated_state.clone();
        persisted_state.state_version = persisted_version;
        self.state_cache.insert(agent_id, persisted_state.clone());

        // 跨 Agent 效果 write-through（与主 intent step 6.15 同构）
        if !collateral_states.is_empty() {
            let mut dead_notifs = Vec::new();
            for (mut c_state, c_version) in collateral_states.drain(..) {
                let dead = !c_state.is_alive;
                c_state.state_version = c_version;
                info!(
                    "[combat] collateral write-through (subsequent): agent={}, node={}, alive={}",
                    c_state.agent_id, c_state.node_id, c_state.is_alive
                );
                self.state_cache.insert(c_state.agent_id, c_state.clone());
                if dead {
                    dead_notifs.push(decay::DeathNotification::new(
                        c_state.agent_id,
                        "combat".to_string(),
                        "在战斗中被杀害".to_string(),
                        c_state.node_id.clone(),
                        tick_id,
                    ));
                }
            }
            if !dead_notifs.is_empty() {
                info!(
                    "Tick {}: {} 个 Agent 被击杀（subsequent），触发善后",
                    tick_id,
                    dead_notifs.len()
                );
                self.handle_deaths(dead_notifs.clone(), tick_id).await;
                for notif in &dead_notifs {
                    if let Err(e) =
                        crate::reward::settle_lifetime(&self.db_pool, notif.agent_id).await
                    {
                        warn!(
                            "[reward] 一生结算失败 (agent={}, tick={}): {}",
                            notif.agent_id, tick_id, e
                        );
                    }
                }
            }
        }

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
        self.send_reactive_world_state(&persisted_state.node_id, tick_id, events)
            .await;

        for (target_id, event) in &event_tuples {
            if let Err(e) = self.broadcast_event(*target_id, event.clone()).await {
                warn!("事件广播失败: target={}, error={}", target_id, e);
            }
        }

        // subsequent 路径：与 process_single_intent step 11 对齐。
        // 历史 bug：action 致死（HP 归零、stamina 归零等）只在主 intent 末尾检测，
        // subsequent intent（pipe_seq > 0）中的死亡漏检，导致 status='active' 卡死、
        // auto_rebirth 永久拒绝。复用 handle_deaths 完成统一善后。
        if !persisted_state.is_alive {
            let death_notif = decay::DeathNotification::new(
                agent_id,
                "action".to_string(),
                format!(
                    "Action 致死 (subsequent pipe_seq={}): {}",
                    pipe_seq, intent.action_type
                ),
                persisted_state.node_id.clone(),
                tick_id,
            );
            info!(
                "[death] action 致死触发善后 (subsequent): agent={}, action={}, pipe_seq={}, tick={}, node={}",
                agent_id, intent.action_type, pipe_seq, tick_id, persisted_state.node_id
            );
            self.handle_deaths(vec![death_notif], tick_id).await;
            // 生存 Reward 一生结算（与主 intent 同构，幂等）
            if let Err(e) = crate::reward::settle_lifetime(&self.db_pool, agent_id).await {
                warn!(
                    "[reward] 一生结算失败 (agent={}, tick={}): {}",
                    agent_id, tick_id, e
                );
            }
        }

        Ok(())
    }

    // ========================================================================
    // Tick 边界处理
    // ========================================================================
}
