//! 状态处理器
//!
//! 协调意图解析、状态变更和事件生成。

use anyhow::Result;
use tracing::{debug, warn};

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
    /// 这是主入口函数，协调整个处理流程
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
    )> {
        let mut actions_executed = 0;
        let executor = ActionExecutor::new(self.db_pool.clone());
        let mut events = Vec::new();
        let mut action_logs = Vec::new();

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

            // 验证意图
            if let Err(e) = self
                .resolver
                .validate_intent(intent, &agent_states[agent_idx], &agent_states)
                .await
            {
                debug!("动作验证失败: agent={}, error={}", intent.agent_id, e);
                continue;
            }

            // 执行动作
            let result = executor.execute(intent, &mut agent_states[agent_idx]);

            // 记录日志
            let action_type = ActionType::new(&result.action_type);

            // 从配置获取动作中文描述
            let action_type_display = crate::game_data::registry::ActionRegistry::get(&result.action_type)
                .map(|config| config.description.clone());

            let mut action_log = AgentAction {
                id: 0,
                tick_id,
                agent_id: intent.agent_id,
                action_type,
                action_type_display,
                action_data: intent.action_data.clone(),
                result: if result.success {
                    ActionResult::Success
                } else {
                    ActionResult::Failed
                },
                result_message: Some(result.message.clone()),
                thought_log: intent.thought_log.clone(),
                observer_thought: intent.observer_thought.clone(),
                narrative: intent.narrative.clone(),
                created_at: chrono::Utc::now(),
            };

            if result.success {
                debug!(
                    "动作执行成功: agent={}, action={}",
                    intent.agent_id, result.action_type
                );

                // 应用状态变更
                let mut all_changes_applied = true;
                for change in &result.state_changes {
                    let mut ctx =
                        MutationContext::new(&self.db_pool, tick_id, result.intent_id, &mut events);

                    // 尝试使用 mutator 处理状态变更
                    let mut applied = false;
                    for mutator in &self.mutators {
                        if let Ok(true) = mutator.mutate(change, &mut agent_states, &mut ctx).await
                        {
                            applied = true;
                            break;
                        }
                    }

                    // 如果没有 mutator 处理，使用回退逻辑
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
                } else {
                    action_log.result = ActionResult::Failed;
                }
            } else {
                warn!(
                    "动作执行失败: agent={}, error={}",
                    intent.agent_id, result.message
                );
                let event = WorldEvent {
                    event_type: WorldEventType::ActionResult,
                    tick_id,
                    description: format!("动作执行失败: {}", result.message),
                    metadata: serde_json::json!({
                        "action": result.action_type,
                        "intent_id": intent.intent_id,
                        "result": "failed",
                        "reason": result.message,
                    }),
                };
                events.push((intent.agent_id, event));
            }

            action_logs.push(action_log);
        }

        Ok((agent_states, actions_executed, events, action_logs))
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
