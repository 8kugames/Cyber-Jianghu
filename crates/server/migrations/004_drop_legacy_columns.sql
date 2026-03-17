-- ============================================================================
-- Migration 004: 删除旧的固定属性列
-- ============================================================================
-- 清理向后兼容逻辑，删除已被 JSONB attributes 替代的旧列
-- 所有代码现在都使用 attributes JSONB 列

-- 删除旧的固定属性列
ALTER TABLE agent_states DROP COLUMN IF EXISTS hp;
ALTER TABLE agent_states DROP COLUMN IF EXISTS stamina;
ALTER TABLE agent_states DROP COLUMN IF EXISTS hunger;
ALTER TABLE agent_states DROP COLUMN IF EXISTS thirst;

-- 添加注释
COMMENT ON COLUMN agent_states.attributes IS '动态属性（JSONB），完全数据驱动，支持任意扩展。旧固定列（hp, stamina, hunger, thirst）已删除';
