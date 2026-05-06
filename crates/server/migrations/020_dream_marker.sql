-- 托梦影响标记：记录哪些 action 受托梦驱动
ALTER TABLE agent_action_logs ADD COLUMN IF NOT EXISTS dream_marker JSONB;
