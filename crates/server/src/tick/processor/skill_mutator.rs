// ============================================================================
// LLM 行为指令 (SKILL.md) 状态变更器
// ============================================================================
//
// 【重要区分】本模块中的 "skill" 一词专指 SKILL.md 元认知框架，
// 不是 RPG 技能、不是战斗加成、不是数值属性。
//
// SKILL.md 是什么：
//   - 元认知框架：帮助 Agent 学会"更像人"的思维方式
//   - 不是"怎么用剑""怎么采药"，而是"怎么评估信任""怎么判断进退"
//   - 存放在 config/skills/{category}/{skill_id}/SKILL.md
//   - 通过 Server ConfigUpdate 推送给 Agent，注入 LLM prompt 上下文
//   - 作用：提供思维工具（"遇到这种情况值得考虑的因素"）
//
// SKILL.md 不是什么：
//   - 不是 RPG 数值技能（没有攻击力+10、防御+5 之类的数值加成）
//   - 不是战斗技能树（没有技能等级、技能点数）
//   - 不影响任何 AgentState 的数值属性（hp/stamina/qi 等完全无关）
//   - 不与 persona 重叠——persona 是"我是谁"，skill 是"我怎么想"
//
// 习得机制（数据驱动）：
//   Agent 执行 action 成功后，按 action category 累计计数。
//   当计数达到 game_rules.yaml 中 skill_acquisition 配置的阈值时，
//   自动触发 StateChange::SkillLearned，无需显式"学习"动作。
//
// 完整数据流：
//   1. Agent 执行 action 成功 → processor.rs 递增 action_counts
//   2. action_counts 达标 → StateChange::SkillLearned { skill_id }
//   3. SkillMutator 将 skill_id 追加到 AgentState.skills: Vec<String>
//   4. 持久化到 DB：JSONB attributes._skills 字段
//   5. realtime.rs 检测到新增技能，推送 ConfigUpdate 给 Agent
//   6. Agent 收到 SkillContent → 更新 skill_cache（内存 + 本地文件）
//   7. Agent 后续决策中从 skill_cache 读取认知框架注入 LLM prompt
//
// 配置加载链：
//   config/skills/{category}/{id}/SKILL.md
//     → skills_loader.rs 解析为 SkillDefinition
//     → SkillRegistry 全局索引
//     → handler.rs 连接时按 agent 已掌握技能过滤推送
//     → realtime.rs 新习得时增量推送
//
// 关联文件索引：
//   - 状态变更定义：actions/types.rs → StateChange::SkillLearned
//   - 注册表：game_data/registry/skill_registry.rs → SkillRegistry
//   - 类型定义：game_data/types/skills.rs → SkillDefinition
//   - 加载器：game_data/loaders/skills_loader.rs → load_skills()
//   - 下发映射：tick/broadcaster.rs → state.skills → SkillInfo
//   - 经验阈值：tick/processor/processor.rs → check_skill_acquisition()
//   - 推送逻辑：tick/realtime.rs → 新增技能时 ConfigUpdate 推送
//   - Agent 消费：agent/soul/actor/engine_prompts.rs → build_skill_instructions()
//   - Agent 工具：agent/soul/earth/skill_tool.rs → skill_view 工具
//   - 协议类型：protocol/types/entities.rs → SkillInfo, SkillContent
//
// 【防误用规则】
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
    use crate::db::DbPool;
    use crate::game_data::init_test_registry;
    use crate::models::AgentState;
    use uuid::Uuid;

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
            skill_id: "social/trust-reading".to_string(),
        };

        let result = mutator.mutate(&change, &mut states, &mut ctx).await;
        assert!(result.unwrap());
        assert!(
            states[0]
                .skills
                .contains(&"social/trust-reading".to_string())
        );
    }

    #[tokio::test]
    async fn test_skill_mutator_idempotent() {
        let mutator = SkillMutator;
        let agent_id = Uuid::new_v4();
        let mut states = vec![make_test_agent(agent_id)];
        states[0].skills.push("social/trust-reading".to_string());
        let mut events = vec![];
        let db_pool = DbPool::connect_lazy("postgres://postgres@localhost/postgres").unwrap();
        let mut ctx = MutationContext::new(&db_pool, 1, None, &mut events);

        let change = StateChange::SkillLearned {
            agent_id,
            skill_id: "social/trust-reading".to_string(),
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
            skill_id: "social/trust-reading".to_string(),
        };

        let result = mutator.mutate(&change, &mut states, &mut ctx).await;
        assert!(!result.unwrap()); // 未找到目标 agent
        assert!(states[0].skills.is_empty());
    }
}
