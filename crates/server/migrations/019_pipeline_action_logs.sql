-- ============================================================================
-- 019_pipeline_action_logs.sql
-- ============================================================================
-- Pipeline 展平记录：每 (agent_id, tick_id, pipe_seq) 一条独立 action log
-- 替代原 (agent_id, tick_id) UNIQUE 约束（单 tick 仅保留一条）
-- ============================================================================

-- 去除原有 UNIQUE 约束
DROP INDEX IF EXISTS idx_agent_action_logs_agent_tick_unique;

-- 新增 pipe_seq 列（pipeline 内序号，避免与 Intent.intent_id: Uuid 命名冲突）
ALTER TABLE agent_action_logs ADD COLUMN IF NOT EXISTS pipe_seq INTEGER NOT NULL DEFAULT 0;

-- 新 UNIQUE 约束：(agent_id, tick_id, pipe_seq)
CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_action_logs_agent_tick_pipeseq_unique
ON agent_action_logs (agent_id, tick_id, pipe_seq);
