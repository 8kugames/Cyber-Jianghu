-- ============================================================================
-- 011_experience_stream_index.sql
-- ============================================================================
-- 为经历日志流水查询添加索引，加速 result + tick_id 全局排序
-- ============================================================================

-- 加速 result = 'success' 过滤 + tick_id DESC 排序的全表扫描
-- 注意：移除了 CONCURRENTLY，因为在 --single-transaction 模式下无法使用
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_result_tick
ON agent_action_logs (result, tick_id DESC);

COMMENT ON INDEX idx_agent_action_logs_result_tick
    IS '加速经历日志流水查询：WHERE result = ''success'' ORDER BY tick_id DESC';