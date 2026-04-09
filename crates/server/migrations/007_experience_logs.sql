-- ============================================================================
-- 007_experience_logs.sql
-- ============================================================================
-- 为 agent_action_logs 表添加：
-- - action_type_display: 动作的中文描述（从 actions.yaml 配置获取）
-- - result_message: 执行结果的详细描述
-- ============================================================================

-- 添加 action_type_display 列（动作中文描述）
-- 注意：实际描述由代码在 processor.rs 中从 ActionRegistry 填充
ALTER TABLE agent_action_logs
ADD COLUMN IF NOT EXISTS action_type_display VARCHAR(200);

COMMENT ON COLUMN agent_action_logs.action_type_display
    IS '动作中文描述，如"休息，不做任何操作"、"公开说话"、"移动到指定位置"';

-- 添加 result_message 列（执行结果详细描述）
-- 注意：实际描述由代码在 processor.rs 中从 ActionExecutionResult.message 填充
ALTER TABLE agent_action_logs
ADD COLUMN IF NOT EXISTS result_message TEXT;

COMMENT ON COLUMN agent_action_logs.result_message
    IS '动作执行结果的详细描述，如"休息后体力恢复了5点"、"成功向李四说话：你好"';

-- 现有数据保持 NULL，由代码层按需补全
-- 不要在此硬编码动作描述映射
