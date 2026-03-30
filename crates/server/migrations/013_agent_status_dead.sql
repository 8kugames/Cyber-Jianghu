-- ============================================================================
-- 013_agent_status_dead.sql - 扩展 Agent 状态支持死亡
-- ============================================================================
--
-- 变更说明：
-- - 扩展 status 约束，支持 'dead' 状态
-- - 死亡和归隐现在可以区分：
--   - retired: 主动归隐（转生）
--   - dead: 被动死亡（饥饿、口渴、战斗等）
--
-- 状态说明：
-- - active: 活跃状态，当前正在使用的角色
-- - retired: 归隐状态，主动转生（保留历史数据）
-- - dead: 死亡状态，被动死亡（饥饿/口渴/战斗）
-- ============================================================================

-- 删除旧约束
ALTER TABLE agents DROP CONSTRAINT IF EXISTS chk_agents_status;

-- 添加新约束：支持 active, retired, dead 三种状态
ALTER TABLE agents
ADD CONSTRAINT chk_agents_status CHECK (status IN ('active', 'retired', 'dead'));

-- 注释
COMMENT ON COLUMN agents.status IS 'Agent状态：active=活跃, retired=归隐（主动）, dead=死亡（被动）';

-- ============================================================================
-- 记录版本
-- ============================================================================
INSERT INTO schema_version (version, description)
VALUES (13, 'Agent Status: Add dead status to distinguish from retired')
ON CONFLICT (version) DO NOTHING;
