-- ============================================================================
-- 010_realtime_action_logs.sql
-- ============================================================================
-- 0.1.0 实时架构改造：
-- 1. 移除 agent_action_logs.tick_id 的 FK 约束（tick_logs 在实时模式下不再是前置条件）
-- 2. 新增 UNIQUE 约束 (agent_id, tick_id)，支持 UPSERT
-- ============================================================================

-- 移除 FK 约束：agent_action_logs.tick_id 不再依赖 tick_logs
ALTER TABLE agent_action_logs
DROP CONSTRAINT IF EXISTS agent_action_logs_tick_id_fkey;

-- 新增 UNIQUE 约束：每个 agent 每个 tick 最多一条 action_log
-- 替换旧的非唯一索引 idx_agent_action_logs_narrative
-- 注意：Pipeline 模式（多 Intent/tick）可能产生重复 (agent_id, tick_id)，
-- 先去重保留最新一条，再创建唯一索引。019 会将此索引替换为含 pipe_seq 的版本。
DROP INDEX IF EXISTS idx_agent_action_logs_narrative;
DELETE FROM agent_action_logs a USING agent_action_logs b
WHERE a.agent_id = b.agent_id AND a.tick_id = b.tick_id AND a.id < b.id;
CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_action_logs_agent_tick_unique
ON agent_action_logs (agent_id, tick_id);

COMMENT ON COLUMN agent_action_logs.tick_id
    IS 'Tick 编号（由墙钟计算，不再依赖 tick_logs 表）';
