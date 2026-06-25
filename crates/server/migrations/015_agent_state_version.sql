-- ============================================================================
-- 015_agent_state_version.sql
-- ============================================================================
-- P0-7: 为 agent_states 引入同 tick 行内乐观锁版本号，消除 UPSERT 静默覆盖
-- ============================================================================

ALTER TABLE agent_states
    ADD COLUMN IF NOT EXISTS state_version BIGINT NOT NULL DEFAULT 0;

COMMENT ON COLUMN agent_states.state_version IS '同一 (agent_id, tick_id) 行内的乐观锁版本号';
