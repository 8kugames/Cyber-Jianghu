-- ============================================================================
-- 017_chaos_marker.sql
-- ============================================================================
-- 为 agent_action_logs 表添加 chaos_marker 列，用于标识混沌行为来源
-- 前端据此渲染"陷入混乱"徽章
-- ============================================================================

ALTER TABLE agent_action_logs
ADD COLUMN IF NOT EXISTS chaos_marker JSONB;

COMMENT ON COLUMN agent_action_logs.chaos_marker
    IS '混沌行为标记：{"type":"Sanity","detail":{"sanity":25}} 或 {"type":"LlmQuotaExhausted","detail":{"consecutive_failures":15}}';