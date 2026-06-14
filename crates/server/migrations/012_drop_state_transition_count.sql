-- 012_drop_state_transition_count.sql
-- v6 根因裁断落地：删除 ProposedActionIR.state_transition_count 字段后，DB schema 残留列清理
--
-- 影响表：action_evolution_proposals
-- 同步状态（v6 实施后）：
--   - protocol/src/types/governance.rs: ProposedActionIR.state_transition_count 已删除
--   - server/src/governance/proposal_store.rs: INSERT/SELECT 已不再引用此列
--   - server/src/governance/{auto_evolve,llm_review,classifier}.rs: 测试 fixture 已更新
--   - server/src/governance/ir_generator.rs: IRGenerator 不输出此字段
--   - server/migrations/010_action_evolution.sql:11: 此列定义残留（本次清理目标）

ALTER TABLE action_evolution_proposals DROP COLUMN IF EXISTS state_transition_count;

-- 同步更新 010 migration 的列注释（保持 schema 文档一致性）
COMMENT ON TABLE action_evolution_proposals IS '动作演化提案原始证据（v6 治理链路）';
