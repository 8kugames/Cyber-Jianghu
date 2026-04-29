//! 状态变更器
//!
//! 定义状态变更的 trait 和具体实现。

use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use crate::actions::StateChange;
use crate::db::DbPool;
use crate::models::{AgentState, WorldEvent};

/// 变更上下文，包含执行状态变更所需的全部依赖
#[allow(dead_code)]
pub struct MutationContext<'a> {
    /// 数据库连接池
    pub db_pool: &'a DbPool,
    /// 当前 Tick ID
    pub tick_id: i64,
    /// 触发此变更的 Intent ID
    pub intent_id: Option<Uuid>,
    /// 事件收集器
    pub events: &'a mut Vec<(Uuid, WorldEvent)>,
}

impl<'a> MutationContext<'a> {
    /// 创建新的变更上下文
    pub fn new(
        db_pool: &'a DbPool,
        tick_id: i64,
        intent_id: Option<Uuid>,
        events: &'a mut Vec<(Uuid, WorldEvent)>,
    ) -> Self {
        Self {
            db_pool,
            tick_id,
            intent_id,
            events,
        }
    }

    /// 添加事件
    #[allow(dead_code)]
    pub fn add_event(&mut self, agent_id: Uuid, event: WorldEvent) {
        self.events.push((agent_id, event));
    }
}

/// 状态变更器
///
/// 负责执行特定类型的状态变更
#[async_trait]
pub trait StateMutator: Send + Sync {
    /// 执行状态变更
    ///
    /// # 参数
    /// - `change`: 状态变更描述
    /// - `states`: 所有 Agent 状态（可变引用）
    /// - `ctx`: 变更上下文（包含 db_pool、tick_id、events）
    ///
    /// # 返回
    /// - `Ok(true)`: 变更成功应用
    /// - `Ok(false)`: 变更无法应用（如目标不存在）
    /// - `Err(...)`: 变更过程中发生错误
    async fn mutate(
        &self,
        change: &StateChange,
        states: &mut [AgentState],
        ctx: &mut MutationContext<'_>,
    ) -> Result<bool>;
}

/// 属性变更器
///
/// 处理 HP、体力、饥饿等属性变更
pub struct AttributeMutator;

#[async_trait]
impl StateMutator for AttributeMutator {
    async fn mutate(
        &self,
        change: &StateChange,
        states: &mut [AgentState],
        _ctx: &mut MutationContext<'_>,
    ) -> Result<bool> {
        match change {
            StateChange::AttributeChanged {
                agent_id,
                attribute,
                delta,
            } => {
                if let Some(state) = states.iter_mut().find(|s| s.agent_id == *agent_id) {
                    // 使用 StatusComponent 应用变更（带范围限制）
                    let delta_i32 = delta.get();
                    let formula_context = state.get_formula_context();

                    if let Ok(_new_val) =
                        state
                            .status
                            .apply_change(attribute, delta_i32, &formula_context)
                    {
                        // 检查死亡条件，仅设置死亡状态，死亡后的清理工作由 scheduler 统一处理
                        if state.status.check_death_condition(attribute) {
                            state.is_alive = false;
                            let _ = state.status.set("hp", 0); // 确保 HP 归零
                            tracing::warn!("Agent {} 因 {} 归零而死亡", agent_id, attribute);
                        }
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            StateChange::HpChanged { agent_id, delta } => {
                if let Some(state) = states.iter_mut().find(|s| s.agent_id == *agent_id) {
                    let formula_context = state.get_formula_context();

                    if let Ok(new_hp) = state.status.apply_change("hp", *delta, &formula_context) {
                        // HP 归零时仅设置死亡状态，死亡后的清理工作由 scheduler 统一处理
                        if new_hp == 0 {
                            state.is_alive = false;
                            tracing::warn!("Agent {} HP 归零而死亡", agent_id);
                        }
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            StateChange::AttributeMaxChanged {
                agent_id,
                attribute,
                delta,
            } => {
                if let Some(state) = states.iter_mut().find(|s| s.agent_id == *agent_id) {
                    if let Ok(new_max) = state.status.apply_max_change(attribute, *delta) {
                        tracing::info!(
                            "Agent {} 属性 {} 上限提升 +{} (new effective max includes +{})",
                            agent_id,
                            attribute,
                            delta,
                            new_max
                        );
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            _ => Ok(false), // 其他类型不处理
        }
    }
}

/// 背包变更器
///
/// 处理物品转移、背包增减等
/// 注意：物品操作由 InventoryManager 直接处理数据库，此 mutator 仅作为占位符
pub struct InventoryMutator;

#[async_trait]
impl StateMutator for InventoryMutator {
    async fn mutate(
        &self,
        _change: &StateChange,
        _states: &mut [AgentState],
        _ctx: &mut MutationContext<'_>,
    ) -> Result<bool> {
        // 物品操作在 apply_state_change 中由 InventoryManager 处理
        // 此 mutator 不处理任何状态变更
        Ok(false)
    }
}

/// 位置变更器
///
/// 处理 Agent 位置移动
pub struct LocationMutator;

#[async_trait]
impl StateMutator for LocationMutator {
    async fn mutate(
        &self,
        change: &StateChange,
        states: &mut [AgentState],
        _ctx: &mut MutationContext<'_>,
    ) -> Result<bool> {
        if let StateChange::LocationChanged {
            agent_id,
            new_location,
            ..
        } = change
        {
            if let Some(state) = states.iter_mut().find(|s| s.agent_id == *agent_id) {
                state.node_id = new_location.clone();
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }
}

/// 技能变更器
///
/// 处理 Agent 技能习得
pub struct SkillMutator;

#[async_trait]
impl StateMutator for SkillMutator {
    async fn mutate(
        &self,
        change: &StateChange,
        states: &mut [AgentState],
        _ctx: &mut MutationContext<'_>,
    ) -> Result<bool> {
        if let StateChange::SkillLearned { agent_id, skill_id } = change {
            if let Some(state) = states.iter_mut().find(|s| s.agent_id == *agent_id) {
                if !state.skills.contains(skill_id) {
                    state.skills.push(skill_id.clone());
                    tracing::info!("Agent {} 习得技能: {}", agent_id, skill_id);
                }
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::init_test_registry;
    use crate::models::AgentState;

    fn make_test_agent(agent_id: Uuid) -> AgentState {
        init_test_registry();
        let mut state = AgentState::new(agent_id, 1);
        state.node_id = "test_location".to_string();
        state.is_alive = true;
        state.inventory_cleared_this_tick = false;
        state.skills = vec![];
        state
    }

    #[tokio::test]
    async fn test_mutation_context() {
        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let mut events = vec![];
        let ctx = MutationContext::new(&db_pool, 1, None, &mut events);

        assert_eq!(ctx.tick_id, 1);
    }

    #[tokio::test]
    async fn test_location_mutator() {
        init_test_registry();
        let mutator = LocationMutator;
        let agent_id = Uuid::new_v4();
        let mut states = vec![make_test_agent(agent_id)];
        let mut events = vec![];
        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let mut ctx = MutationContext::new(&db_pool, 1, None, &mut events);

        let change = StateChange::LocationChanged {
            agent_id,
            old_location: "test_location".to_string(),
            new_location: "new_location".to_string(),
        };

        let result = mutator.mutate(&change, &mut states, &mut ctx).await;
        assert!(result.unwrap());
        // LocationMutator 只标记成功，实际更新在 apply_state_change 中处理
    }

    #[tokio::test]
    async fn test_skill_mutator_learn() {
        let mutator = SkillMutator;
        let agent_id = Uuid::new_v4();
        let mut states = vec![make_test_agent(agent_id)];
        let mut events = vec![];
        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let mut ctx = MutationContext::new(&db_pool, 1, None, &mut events);

        let change = StateChange::SkillLearned {
            agent_id,
            skill_id: "martial/sword-basic".to_string(),
        };

        let result = mutator.mutate(&change, &mut states, &mut ctx).await;
        assert!(result.unwrap());
        assert!(
            states[0]
                .skills
                .contains(&"martial/sword-basic".to_string())
        );
    }

    #[tokio::test]
    async fn test_skill_mutator_idempotent() {
        let mutator = SkillMutator;
        let agent_id = Uuid::new_v4();
        let mut states = vec![make_test_agent(agent_id)];
        states[0].skills.push("martial/sword-basic".to_string());
        let mut events = vec![];
        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let mut ctx = MutationContext::new(&db_pool, 1, None, &mut events);

        let change = StateChange::SkillLearned {
            agent_id,
            skill_id: "martial/sword-basic".to_string(),
        };

        let result = mutator.mutate(&change, &mut states, &mut ctx).await;
        assert!(result.unwrap());
        assert_eq!(states[0].skills.len(), 1); // 无重复
    }

    #[tokio::test]
    async fn test_skill_mutator_wrong_agent() {
        let mutator = SkillMutator;
        let agent_id = Uuid::new_v4();
        let other_id = Uuid::new_v4();
        let mut states = vec![make_test_agent(agent_id)];
        let mut events = vec![];
        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let mut ctx = MutationContext::new(&db_pool, 1, None, &mut events);

        let change = StateChange::SkillLearned {
            agent_id: other_id,
            skill_id: "martial/sword-basic".to_string(),
        };

        let result = mutator.mutate(&change, &mut states, &mut ctx).await;
        assert!(!result.unwrap()); // 未找到目标 agent
        assert!(states[0].skills.is_empty());
    }
}
