-- ============================================================================
-- Migration 002: 使用 JSONB 实现完全动态属性系统
-- ============================================================================
-- 此迁移将 agent_states 表从固定列改为 JSONB 存储所有属性
-- 实现真正的数据驱动 COI 架构
--
-- 注意：旧的固定列（hp, stamina, hunger, thirst）将在 Migration 004 中删除

-- 添加新的 JSONB 列
ALTER TABLE agent_states ADD COLUMN attributes JSONB;

-- 迁移现有数据到 JSONB 格式
UPDATE agent_states
SET attributes = jsonb_build_object(
    'hp', hp,
    'stamina', stamina,
    'hunger', hunger,
    'thirst', thirst
)
WHERE attributes IS NULL;

-- 设置 attributes 列为 NOT NULL（数据迁移后）
ALTER TABLE agent_states ALTER COLUMN attributes SET NOT NULL;

-- 为 JSONB 创建 GIN 索引以支持高效的属性查询
CREATE INDEX IF NOT EXISTS idx_agent_states_attributes_gin ON agent_states USING GIN (attributes);

-- 创建属性查询的辅助索引（常用属性）
CREATE INDEX IF NOT EXISTS idx_agent_states_attributes_hp ON agent_states ((attributes->>'hp')::int);
CREATE INDEX IF NOT EXISTS idx_agent_states_attributes_hunger ON agent_states ((attributes->>'hunger')::int);
CREATE INDEX IF NOT EXISTS idx_agent_states_attributes_thirst ON agent_states ((attributes->>'thirst')::int);
CREATE INDEX IF NOT EXISTS idx_agent_states_attributes_stamina ON agent_states ((attributes->>'stamina')::int);

-- 添加注释
COMMENT ON COLUMN agent_states.attributes IS '动态属性（JSONB），完全数据驱动，支持任意扩展';
