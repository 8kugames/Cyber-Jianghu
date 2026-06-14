-- 013_drop_ir_columns.sql
-- 精简 action_evolution_proposals schema：删除 IR 字段
--
-- 业务背景：
--   提案触发条件是 agent 端 UnknownAction，agent 无可信执行特征数据源。
--   原 IR 字段（actor_arity/target_arity/tick_span/phase_count/protocol_kind/
--   effect_refs/requirement_refs）全部由 server 端 placeholder 填充，
--   零信息量且引入反序列化静默 fallback 风险（P0.2 修复历史）。
--
-- 新流程：
--   actions.yaml 是运行时真相。agent 提议时只传 intent 上下文（action_data），
--   伏羲 LLM 审议时推断原子性与执行特征，approve 后由 auto_evolve 写入
--   actions.yaml，DB 中不再存 IR。
--
-- 影响：
--   - server/governance/proposal_store.rs: insert/get_proposal 已同步移除 IR 字段
--   - server/governance/types.rs: ProposalEvidence.ir 字段已删除，新增 action_data
--   - protocol/types/governance.rs: ProposedActionIR / IRSource 类型已删除

ALTER TABLE action_evolution_proposals
    DROP COLUMN IF EXISTS actor_arity,
    DROP COLUMN IF EXISTS target_arity,
    DROP COLUMN IF EXISTS tick_span,
    DROP COLUMN IF EXISTS phase_count,
    DROP COLUMN IF EXISTS protocol_kind,
    DROP COLUMN IF EXISTS effect_refs,
    DROP COLUMN IF EXISTS requirement_refs;

ALTER TABLE action_evolution_proposals
    ADD COLUMN IF NOT EXISTS action_data JSONB NOT NULL DEFAULT '{}'::jsonb;

COMMENT ON COLUMN action_evolution_proposals.action_data
    IS 'Agent intent 完整参数（target/item/quantity 等），供伏羲 LLM 审议';
