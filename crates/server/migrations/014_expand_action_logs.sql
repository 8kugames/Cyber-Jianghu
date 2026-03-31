-- ============================================================================
-- Migration: 014_expand_action_logs
-- Description: 扩展 agent_action_logs 表，添加叙事化字段
-- ============================================================================

-- 添加 thought_log 字段（ActorSoul 的思考日志）
ALTER TABLE agent_action_logs ADD COLUMN IF NOT EXISTS thought_log TEXT;

-- 添加 observer_thought 字段（ReflectorSoul 的审查理由）
ALTER TABLE agent_action_logs ADD COLUMN IF NOT EXISTS observer_thought TEXT;

-- 添加 narrative 字段（叙事化经历描述）
ALTER TABLE agent_action_logs ADD COLUMN IF NOT EXISTS narrative TEXT;

-- 为新字段添加索引（便于按 agent_id + tick_id 查询叙事）
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_narrative ON agent_action_logs(agent_id, tick_id DESC);

-- 注释
COMMENT ON COLUMN agent_action_logs.thought_log IS 'ActorSoul 思考日志';
COMMENT ON COLUMN agent_action_logs.observer_thought IS 'ReflectorSoul 审查理由';
COMMENT ON COLUMN agent_action_logs.narrative IS '叙事化经历描述';

-- ============================================================================
-- 记录版本
-- ============================================================================
INSERT INTO schema_version (version, description)
VALUES (14, 'Expand agent_action_logs: add thought_log, observer_thought, narrative')
ON CONFLICT (version) DO NOTHING;
