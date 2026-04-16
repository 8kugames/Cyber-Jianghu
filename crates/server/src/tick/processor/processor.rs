//! 状态处理器
//!
//! 协调意图解析、状态变更和事件生成。

use anyhow::Result;
use std::collections::HashMap;
use tracing::warn;

use super::{
    executor::apply_state_change,
    mutator::{MutationContext, StateMutator},
    resolver::IntentResolver,
};
use crate::actions::ActionExecutor;
use crate::db::DbPool;
use crate::models::{
    ActionResult, ActionType, AgentAction, AgentState, Intent, WorldEvent, WorldEventType,
};
use cyber_jianghu_protocol::{ExecutionSummary, IntentExecutionResult, IntentExecutionStatus};

/// 单条 Intent 处理结果
pub struct SingleProcessingResult {
    /// 更新后的 Agent 状态
    pub updated_state: AgentState,
    /// 生成的事件列表
    pub events: Vec<(uuid::Uuid, WorldEvent)>,
}

/// 状态处理器
///
/// 负责协调意图结算和状态变更
pub struct StateProcessor {
    /// 意图解析器
    resolver: IntentResolver,
    /// 状态变更器列表
    mutators: Vec<Box<dyn StateMutator>>,
    /// 数据库连接池
    db_pool: DbPool,
}

impl StateProcessor {
    /// 创建新的处理器
    pub fn new(db_pool: DbPool) -> Self {
        Self {
            resolver: IntentResolver::new(db_pool.clone()),
            mutators: vec![
                Box::new(super::mutator::AttributeMutator),
                Box::new(super::mutator::InventoryMutator),
                Box::new(super::mutator::LocationMutator),
            ],
            db_pool,
        }
    }

    /// 添加状态变更器
    #[allow(dead_code)]
    pub fn with_mutator<M: StateMutator + 'static>(mut self, mutator: M) -> Self {
        self.mutators.push(Box::new(mutator));
        self
    }

    /// 处理意图列表
    ///
    /// 这是主入口函数，协调整个处理流程。
    /// 支持多 Intent Pipeline 执行：主 Intent 成功后依次执行 subsequent_intents。
    /// 返回值包含 `execution_summaries`：每个 Agent 的 Pipeline 执行汇总，
    /// 由 scheduler 在下次广播时附加到 WorldState。
    pub async fn process_intents(
        &self,
        tick_id: i64,
        mut agent_states: Vec<AgentState>,
        intents: &[Intent],
    ) -> Result<(
        Vec<AgentState>,
        usize,
        Vec<(uuid::Uuid, WorldEvent)>,
        Vec<AgentAction>,
        Vec<(uuid::Uuid, String)>,
        HashMap<uuid::Uuid, ExecutionSummary>,
    )> {
        let mut actions_executed = 0;
        let executor = ActionExecutor::new(self.db_pool.clone());
        let mut events = Vec::new();
        let mut action_logs = Vec::new();
        let mut validation_errors: Vec<(uuid::Uuid, String)> = Vec::new();
        let mut execution_summaries: HashMap<uuid::Uuid, ExecutionSummary> = HashMap::new();

        // 遍历所有意图
        for intent in intents {
            // 校验 tick_id
            if intent.tick_id != tick_id {
                warn!(
                    "意图 tick_id 不匹配: agent={}, intent_tick={}, current_tick={}",
                    intent.agent_id, intent.tick_id, tick_id
                );
                continue;
            }

            // 更新在线时间
            if let Err(e) = crate::db::update_agent_online(&self.db_pool, intent.agent_id).await {
                warn!("更新 Agent {} 在线时间失败: {}", intent.agent_id, e);
            }

            // 查找 Agent
            let agent_idx = match agent_states
                .iter()
                .position(|s| s.agent_id == intent.agent_id)
            {
                Some(idx) => idx,
                None => {
                    warn!("意图来自未知 Agent: {}", intent.agent_id);
                    continue;
                }
            };

            // 构建 Pipeline intent 列表
            let pipeline_intents = intent.as_pipeline();
            let mut pipeline_results: Vec<IntentExecutionResult> = Vec::new();
            let mut pipeline_failed = false;

            // Sagas: 执行前快照 agent_state + events 长度，Pipeline 失败时回滚
            let agent_state_snapshot = agent_states[agent_idx].clone();
            let events_len_before = events.len();

            for (pipe_idx, pipe_intent) in pipeline_intents.iter().enumerate() {
                if pipeline_failed {
                    // 前置 Intent 失败，跳过后续
                    pipeline_results.push(IntentExecutionResult {
                        intent_id: pipe_intent.intent_id,
                        status: IntentExecutionStatus::Skipped,
                        executed_quantity: None,
                        error_reason: Some("前置Intent失败".to_string()),
                    });
                    continue;
                }

                // Pipeline 完整性校验（后续 intent 必须属于同一 agent 和 tick）
                if pipe_idx > 0
                    && (pipe_intent.agent_id != intent.agent_id || pipe_intent.tick_id != tick_id)
                {
                    pipeline_results.push(IntentExecutionResult {
                        intent_id: pipe_intent.intent_id,
                        status: IntentExecutionStatus::Failed,
                        executed_quantity: None,
                        error_reason: Some("Pipeline完整性校验失败".to_string()),
                    });
                    pipeline_failed = true;
                    continue;
                }

                // 验证意图（基于当前状态，已随执行更新）
                if let Err(e) = self
                    .resolver
                    .validate_intent(pipe_intent, &agent_states[agent_idx], &agent_states)
                    .await
                {
                    warn!(
                        "Pipeline intent {} 验证失败: agent={}, error={}",
                        pipe_idx, pipe_intent.agent_id, e
                    );
                    pipeline_results.push(IntentExecutionResult {
                        intent_id: pipe_intent.intent_id,
                        status: IntentExecutionStatus::Failed,
                        executed_quantity: None,
                        error_reason: Some(format!("{}", e)),
                    });
                    // 主 Intent 验证失败走原有逻辑
                    if pipe_idx == 0 {
                        validation_errors.push((intent.agent_id, format!("{}", e)));
                    }
                    pipeline_failed = true;
                    continue;
                }

                // 执行动作
                let result = executor.execute(pipe_intent, &mut agent_states[agent_idx]);

                if result.success {
                    // 应用状态变更
                    let mut all_changes_applied = true;
                    for change in &result.state_changes {
                        let mut ctx = MutationContext::new(
                            &self.db_pool,
                            tick_id,
                            result.intent_id,
                            &mut events,
                        );

                        let mut applied = false;
                        for mutator in &self.mutators {
                            if let Ok(true) =
                                mutator.mutate(change, &mut agent_states, &mut ctx).await
                            {
                                applied = true;
                                break;
                            }
                        }

                        if !applied {
                            applied = apply_state_change(
                                &self.db_pool,
                                tick_id,
                                change,
                                result.intent_id,
                                &mut agent_states,
                                &mut events,
                            )
                            .await;
                        }

                        if !applied {
                            all_changes_applied = false;
                        }
                    }

                    if all_changes_applied {
                        actions_executed += 1;
                        pipeline_results.push(IntentExecutionResult {
                            intent_id: pipe_intent.intent_id,
                            status: IntentExecutionStatus::Success,
                            executed_quantity: None,
                            error_reason: None,
                        });
                    } else {
                        pipeline_results.push(IntentExecutionResult {
                            intent_id: pipe_intent.intent_id,
                            status: IntentExecutionStatus::Failed,
                            executed_quantity: None,
                            error_reason: Some("状态变更应用失败".to_string()),
                        });
                        pipeline_failed = true;
                    }
                } else {
                    pipeline_results.push(IntentExecutionResult {
                        intent_id: pipe_intent.intent_id,
                        status: IntentExecutionStatus::Failed,
                        executed_quantity: None,
                        error_reason: Some(result.message.clone()),
                    });
                    pipeline_failed = true;
                }
            }

            // Sagas: Pipeline 失败时回滚到快照（内存状态 + events 全部撤销）
            // NOTE: apply_state_change 中通过 InventoryManager 写入的 DB 操作不会被回滚。
            // 但下一个 tick 的持久化会用回滚后的内存状态覆盖 DB，保证最终一致性。
            // 完整事务性回滚需要将 executor 改为接收 Transaction——代价过大，延后处理。
            //
            // TODO: 跟踪最终一致性边界情况
            // - 验证每个 tick 的持久化确实使用回滚后的内存状态
            // - 添加集成测试验证 Pipeline 失败 → 回滚 → 下一个 tick 持久化的完整流程
            if pipeline_failed {
                agent_states[agent_idx] = agent_state_snapshot;
                events.truncate(events_len_before);
            }

            // 记录主 Intent 的日志
            let action_type = ActionType::new(intent.action_type.as_str());
            let action_type_display =
                crate::game_data::registry::ActionRegistry::get(intent.action_type.as_str())
                    .map(|config| config.name.clone());

            let main_success = pipeline_results
                .first()
                .map(|r| r.status == IntentExecutionStatus::Success)
                .unwrap_or(false);

            let action_log = AgentAction {
                id: 0,
                tick_id,
                agent_id: intent.agent_id,
                action_type,
                action_type_display,
                action_data: intent.action_data.clone(),
                result: if main_success {
                    ActionResult::Success
                } else {
                    ActionResult::Failed
                },
                result_message: pipeline_results
                    .first()
                    .and_then(|r| r.error_reason.clone())
                    .or_else(|| {
                        if main_success {
                            Some("Pipeline执行成功".to_string())
                        } else {
                            None
                        }
                    }),
                thought_log: intent.thought_log.clone(),
                observer_thought: intent.observer_thought.clone(),
                narrative: intent.narrative.clone(),
                soul_cycle_metadata: None,
                created_at: chrono::Utc::now(),
            };

            // 如果主 Intent 执行失败，生成失败事件
            if !main_success {
                let reason = pipeline_results
                    .first()
                    .and_then(|r| r.error_reason.clone())
                    .unwrap_or_else(|| "未知原因".to_string());
                let event = WorldEvent {
                    event_type: WorldEventType::ActionResult,
                    tick_id,
                    description: format!("动作执行失败: {}", reason),
                    metadata: serde_json::json!({
                        "action": intent.action_type.as_str(),
                        "intent_id": intent.intent_id,
                        "result": "failed",
                        "reason": reason,
                    }),
                };
                events.push((intent.agent_id, event));
            }

            action_logs.push(action_log);

            // 存储 ExecutionSummary（始终插入，单意图成功也需记录）
            let summary = ExecutionSummary::from_results(&pipeline_results);
            execution_summaries.insert(intent.agent_id, summary);
        }

        Ok((
            agent_states,
            actions_executed,
            events,
            action_logs,
            validation_errors,
            execution_summaries,
        ))
    }

    /// 处理单条 Intent（实时模式用）
    ///
    /// 与 `process_intents()` 核心逻辑一致，但作用于单个 Agent + 单条 Intent。
    /// 保留 Sagas 快照/回滚机制。
    pub async fn process_single_intent(
        &self,
        tick_id: i64,
        mut agent_state: AgentState,
        intent: &Intent,
        all_states: &[AgentState],
    ) -> Result<SingleProcessingResult> {
        let executor = ActionExecutor::new(self.db_pool.clone());
        let mut events: Vec<(uuid::Uuid, WorldEvent)> = Vec::new();

        // 更新在线时间
        if let Err(e) = crate::db::update_agent_online(&self.db_pool, intent.agent_id).await {
            warn!("更新 Agent {} 在线时间失败: {}", intent.agent_id, e);
        }

        // 构建 Pipeline
        let pipeline_intents = intent.as_pipeline();
        let mut pipeline_failed = false;

        // Sagas: 快照
        let agent_state_snapshot = agent_state.clone();
        let events_len_before = events.len();

        for (pipe_idx, pipe_intent) in pipeline_intents.iter().enumerate() {
            if pipeline_failed {
                break;
            }

            // Pipeline 完整性校验
            if pipe_idx > 0
                && (pipe_intent.agent_id != intent.agent_id || pipe_intent.tick_id != tick_id)
            {
                pipeline_failed = true;
                continue;
            }

            // 验证（传入所有 Agent 状态，支持跨 Agent 校验如 attack/trade）
            if let Err(e) = self
                .resolver
                .validate_intent(pipe_intent, &agent_state, all_states)
                .await
            {
                warn!(
                    "Pipeline intent {} 验证失败: agent={}, error={}",
                    pipe_idx, pipe_intent.agent_id, e
                );
                pipeline_failed = true;
                continue;
            }

            // 执行
            let result = executor.execute(pipe_intent, &mut agent_state);

            if result.success {
                let mut all_applied = true;
                for change in &result.state_changes {
                    let mut ctx =
                        MutationContext::new(&self.db_pool, tick_id, result.intent_id, &mut events);

                    // 构造单 Agent 的 slice 供 mutator 使用
                    let mut single_states = vec![agent_state.clone()];
                    let mut applied = false;
                    for mutator in &self.mutators {
                        if let Ok(true) = mutator.mutate(change, &mut single_states, &mut ctx).await
                        {
                            applied = true;
                            agent_state = single_states.into_iter().next().unwrap_or(agent_state);
                            break;
                        }
                    }

                    if !applied {
                        let mut single_states = vec![agent_state.clone()];
                        applied = apply_state_change(
                            &self.db_pool,
                            tick_id,
                            change,
                            result.intent_id,
                            &mut single_states,
                            &mut events,
                        )
                        .await;
                        if applied {
                            agent_state = single_states.into_iter().next().unwrap_or(agent_state);
                        }
                    }

                    if !applied {
                        all_applied = false;
                    }
                }

                if !all_applied {
                    pipeline_failed = true;
                }
            } else {
                pipeline_failed = true;
            }
        }

        // Sagas: 回滚
        if pipeline_failed {
            agent_state = agent_state_snapshot;
            events.truncate(events_len_before);
        }

        // 记录 action log（异步，不阻塞主流程）
        let action_type = ActionType::new(intent.action_type.as_str());
        let action_log = AgentAction {
            id: 0,
            tick_id,
            agent_id: intent.agent_id,
            action_type,
            action_type_display: crate::game_data::registry::ActionRegistry::get(
                intent.action_type.as_str(),
            )
            .map(|config| config.name.clone()),
            action_data: intent.action_data.clone(),
            result: if pipeline_failed {
                ActionResult::Failed
            } else {
                ActionResult::Success
            },
            result_message: None,
            thought_log: intent.thought_log.clone(),
            observer_thought: intent.observer_thought.clone(),
            narrative: intent.narrative.clone(),
            soul_cycle_metadata: None,
            created_at: chrono::Utc::now(),
        };

        let pool = self.db_pool.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::db::batch_insert_action_logs(&pool, &[action_log]).await {
                warn!("Action log 异步写入失败: {}", e);
            }
        });

        Ok(SingleProcessingResult {
            updated_state: agent_state,
            events,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_processor_creation() {
        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let processor = StateProcessor::new(db_pool);
        // 测试创建成功
        assert!(processor.mutators.len() >= 3);
    }
}
