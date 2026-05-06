//! 状态处理器
//!
//! 协调意图解析、状态变更和事件生成。

use anyhow::Result;
use tracing::warn;

use super::{
    executor::apply_state_change,
    mutator::{MutationContext, StateMutator},
    resolver::IntentResolver,
};
use crate::actions::ActionExecutor;
use crate::actions::StateChange;
use crate::db::DbPool;
use crate::game_data::registry::ActionRegistry;
use crate::models::{ActionResult, ActionType, AgentAction, AgentState, Intent, WorldEvent};

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
                Box::new(super::skill_mutator::SkillMutator),
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

    /// 处理单条 Intent（实时模式）
    ///
    /// 单个 Agent + 单条 Intent，保留 Sagas 快照/回滚机制。
    /// pipeline 的 subsequent 逐条处理由 realtime.rs 在每次 DashMap 刷新后调用。
    pub async fn process_single_intent(
        &self,
        tick_id: i64,
        mut agent_state: AgentState,
        intent: &Intent,
        all_states: &[AgentState],
        pipe_seq: i32,
    ) -> Result<SingleProcessingResult> {
        let executor = ActionExecutor::new(self.db_pool.clone());
        let mut events: Vec<(uuid::Uuid, WorldEvent)> = Vec::new();

        // 更新在线时间
        if let Err(e) = crate::db::update_agent_online(&self.db_pool, intent.agent_id).await {
            warn!("更新 Agent {} 在线时间失败: {}", intent.agent_id, e);
        }

        // Sagas: 快照
        let agent_state_snapshot = agent_state.clone();
        let events_len_before = events.len();
        let mut execution_failed = false;

        // 验证（传入所有 Agent 状态，支持跨 Agent 校验如 attack/trade）
        if let Err(e) = self
            .resolver
            .validate_intent(intent, &agent_state, all_states)
            .await
        {
            warn!("Intent 验证失败: agent={}, error={}", intent.agent_id, e);
            execution_failed = true;
        }

        // 执行
        if !execution_failed {
            let result = executor.execute(intent, &mut agent_state);

            if result.success {
                // 经验阈值：按 action category 递增计数 + 检查技能习得
                if let Some(config) = ActionRegistry::get(intent.action_type.as_str()) {
                    let count = agent_state
                        .action_counts
                        .entry(config.category.clone())
                        .or_insert(0);
                    *count += 1;
                }

                // 检查技能习得阈值（基于已更新的 action_counts）
                let acquired_skills = check_skill_acquisition(&agent_state);

                let mut all_applied = true;
                for change in &result.state_changes {
                    let mut ctx =
                        MutationContext::new(&self.db_pool, tick_id, result.intent_id, &mut events);

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

                // 处理经验阈值触发的技能习得
                for skill_id in acquired_skills {
                    let change = StateChange::SkillLearned {
                        agent_id: intent.agent_id,
                        skill_id: skill_id.clone(),
                    };
                    let mut ctx =
                        MutationContext::new(&self.db_pool, tick_id, result.intent_id, &mut events);
                    let mut single_states = vec![agent_state.clone()];
                    for mutator in &self.mutators {
                        if let Ok(true) = mutator.mutate(&change, &mut single_states, &mut ctx).await
                        {
                            agent_state = single_states.into_iter().next().unwrap_or(agent_state);
                            break;
                        }
                    }
                }

                if !all_applied {
                    execution_failed = true;
                }
            } else {
                execution_failed = true;
            }
        }

        // Sagas: 回滚
        if execution_failed {
            agent_state = agent_state_snapshot;
            events.truncate(events_len_before);
        }

        // 单条 Action log
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
            result: if execution_failed {
                ActionResult::Failed
            } else {
                ActionResult::Success
            },
            result_message: None,
            thought_log: intent.thought_log.clone(),
            observer_thought: intent.observer_thought.clone(),
            narrative: intent.narrative.clone(),
            soul_cycle_metadata: None,
            chaos_marker: intent
                .chaos_marker
                .as_ref()
                .and_then(|m| serde_json::to_value(m).ok()),
            created_at: chrono::Utc::now(),
            pipe_seq,
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

/// 检查 Agent 是否因 action category 计数达标而习得新技能
///
/// 遍历 skill_acquisition 配置，对每个尚未掌握的技能，
/// 检查其 trigger_categories 对应的 action_counts 是否都达到 min_count。
fn check_skill_acquisition(agent_state: &AgentState) -> Vec<String> {
    let gd = match crate::game_data::registry() {
        Some(cache) => cache.get(),
        None => return Vec::new(),
    };

    let acquisition_cfg = &gd.game_rules.data.skill_acquisition;
    if acquisition_cfg.is_empty() {
        return Vec::new();
    }

    let mut acquired = Vec::new();
    for (skill_id, entry) in acquisition_cfg {
        // 跳过已掌握的技能
        if agent_state.skills.contains(skill_id) {
            continue;
        }

        // 检查每个 trigger_category 的计数是否达标
        let total: i32 = entry
            .trigger_categories
            .iter()
            .map(|cat| *agent_state.action_counts.get(cat).unwrap_or(&0))
            .sum();

        if total >= entry.min_count {
            acquired.push(skill_id.clone());
        }
    }

    acquired
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
