// ============================================================================
// LLM 行为指令 (SKILL.md) 状态变更器
// ============================================================================
//
// 【重要区分】本模块中的 "skill" 一词专指 SKILL.md 行为指令系统，
// 不是 RPG 技能、不是战斗加成、不是数值属性。
//
// SKILL.md 是什么：
//   - 一份 Markdown 格式的行为指引文档（YAML frontmatter + markdown body）
//   - 存放在 config/skills/{category}/{skill_id}/SKILL.md
//   - 被 Agent 的 LLM 读取后注入到 prompt 上下文中
//   - 作用：改变 Agent 的行为模式（如学会"讨价还价"后交易时更精明）
//
// SKILL.md 不是什么：
//   - 不是 RPG 数值技能（没有攻击力+10、防御+5 之类的数值加成）
//   - 不是战斗技能树（没有技能等级、技能点数）
//   - 不影响任何 AgentState 的数值属性（hp/stamina/qi 等完全无关）
//
// 完整数据流：
//   1. 某个 action 触发 StateChange::SkillLearned { skill_id }
//   2. SkillMutator 将 skill_id 追加到 AgentState.skills: Vec<String>
//   3. 持久化到 DB：JSONB attributes._skills 字段
//   4. broadcaster.rs 读取 state.skills，映射为 SkillInfo { skill_id, name }
//   5. Server 通过 WorldState.skills 下发给 Agent
//   6. Agent engine_prompts.rs 读取对应 SKILL.md 文件，注入 LLM prompt
//   7. LLM 在后续决策中遵循 SKILL.md 中的行为指引
//
// 配置加载链：
//   config/skills/{category}/{id}/SKILL.md
//     → skills_loader.rs 解析为 SkillDefinition
//     → SkillRegistry 全局索引
//     → broadcaster.rs 查询 skill_id → SkillInfo
//
// 关联文件索引：
//   - 状态变更定义：actions/types.rs → StateChange::SkillLearned
//   - 注册表：game_data/registry/skill_registry.rs → SkillRegistry
//   - 类型定义：game_data/types/skills.rs → SkillDefinition
//   - 加载器：game_data/loaders/skills_loader.rs → load_skills()
//   - 下发映射：tick/broadcaster.rs → state.skills → SkillInfo
//   - Agent 消费：agent/soul/actor/engine_prompts.rs → build_skill_instructions()
//   - Agent 工具：agent/soul/earth/skill_tool.rs → skill_view 工具
//   - 协议类型：protocol/types/entities.rs → SkillInfo, SkillContent
//
// 【防误用规则】
//   - 任何新 action 需要触发 SkillLearned 时，必须在 action 描述中明确说明
//     "本动作注入 LLM 行为指令（SKILL.md），不产生任何数值属性变更"
//   - 禁止将 SkillLearned 与 修炼/qi 增长/属性提升 混合使用
//   - AgentState.skills 是纯字符串列表，仅用于 SKILL.md 查找索引
// ============================================================================

use anyhow::Result;
use async_trait::async_trait;

use crate::actions::StateChange;
use crate::models::AgentState;

use super::mutator::{MutationContext, StateMutator};

/// LLM 行为指令变更器
///
/// 将 SKILL.md 的 skill_id 注册到 AgentState.skills 列表中。
/// 这仅是一个"凭证"——表示该 Agent 有权在 prompt 中注入对应 SKILL.md 的内容。
///
/// 实际的 SKILL.md 内容注入发生在 Agent 侧（engine_prompts.rs），
/// 而非 Server 侧。Server 只负责记录"谁掌握了什么行为指令"。
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
                    tracing::info!("Agent {} 获得 LLM 行为指令: {}", agent_id, skill_id);
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
    use uuid::Uuid;
    use crate::db::DbPool;
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
