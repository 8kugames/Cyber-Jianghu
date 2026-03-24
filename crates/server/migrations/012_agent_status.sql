-- ============================================================================
-- 012_agent_status.sql - Agent 状态管理（转生系统）
-- ============================================================================
--
-- 变更说明：
-- - agents 表添加 status 字段，支持 active/retired 状态
-- - 添加 retired_at 时间戳，记录归隐时间
-- - 转生时不再删除记录，而是标记为 retired
-- - 支持查看历史角色
--
-- 状态说明：
-- - active: 活跃状态，当前正在使用的角色
-- - retired: 归隐状态，历史角色（可查看但不可操作）
-- ============================================================================

-- 添加 status 字段
ALTER TABLE agents
ADD COLUMN IF NOT EXISTS status VARCHAR(20) NOT NULL DEFAULT 'active';

-- 添加 retired_at 时间戳
ALTER TABLE agents
ADD COLUMN IF NOT EXISTS retired_at TIMESTAMPTZ;

-- 添加约束：status 只能是 active 或 retired
ALTER TABLE agents
DROP CONSTRAINT IF EXISTS chk_agents_status;
ALTER TABLE agents
ADD CONSTRAINT chk_agents_status CHECK (status IN ('active', 'retired'));

-- 创建索引：按状态查询
CREATE INDEX IF NOT EXISTS idx_agents_status ON agents(status);

-- 创建索引：按设备和状态查询（用于检查是否有活跃角色）
CREATE INDEX IF NOT EXISTS idx_agents_device_status ON agents(device_id, status);

-- 注释
COMMENT ON COLUMN agents.status IS 'Agent状态：active=活跃, retired=归隐';
COMMENT ON COLUMN agents.retired_at IS '归隐时间（转生时设置）';

-- ============================================================================
-- 记录版本
-- ============================================================================
INSERT INTO schema_version (version, description)
VALUES (12, 'Agent Status: Add status field for rebirth system')
ON CONFLICT (version) DO NOTHING;
