-- ============================================================================
-- 008_soul_cycle_metadata.sql
-- ============================================================================
-- 为 agent_action_logs 表添加三魂循环元数据列
-- 使 server-web 能展示与 agent-web 完全相同的三魂详情
-- ============================================================================

-- 三魂循环元数据（JSONB 存储）
-- 由 agent 通过 WebSocket SoulCycleReport 消息上报
ALTER TABLE agent_action_logs
ADD COLUMN IF NOT EXISTS soul_cycle_metadata JSONB;

COMMENT ON COLUMN agent_action_logs.soul_cycle_metadata
    IS '三魂循环完整元数据 JSONB，含人魂结构化 Intent、天魂三层审查结果、即时通道意图';

-- 创建索引加速按 tick_id 查询
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_soul_cycle_metadata
ON agent_action_logs (agent_id, tick_id DESC)
WHERE soul_cycle_metadata IS NOT NULL;

-- 现有数据保持 NULL，由代码层按需补全
