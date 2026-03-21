-- ============================================================================
-- 006_ground_items.sql - 地面物品系统
-- ============================================================================
--
-- 包含：
-- - ground_items: 地面物品表（记录场景中掉落的物品）
--
-- 设计说明：
-- - 物品可以掉落在地面上供其他 Agent 拾取
-- - 支持 node_id 索引以快速查询某个地点的物品
-- ============================================================================

-- ============================================================================
-- ground_items 表
-- ============================================================================
CREATE TABLE IF NOT EXISTS ground_items (
    -- 记录 ID
    id BIGSERIAL PRIMARY KEY,

    -- 所在节点 ID
    node_id VARCHAR(100) NOT NULL,

    -- 物品 ID（外键关联 items 表）
    item_id VARCHAR(50) NOT NULL REFERENCES items(item_id),

    -- 物品数量
    quantity INTEGER NOT NULL DEFAULT 1 CHECK (quantity > 0),

    -- 掉落者（可选，外键关联 agents 表）
    dropped_by UUID REFERENCES agents(agent_id) ON DELETE SET NULL,

    -- 创建时间
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_ground_items_node_id ON ground_items(node_id);
CREATE INDEX IF NOT EXISTS idx_ground_items_item_id ON ground_items(item_id);

-- 注释
COMMENT ON TABLE ground_items IS '地面物品表，记录场景中掉落的物品';
COMMENT ON COLUMN ground_items.node_id IS '所在节点ID';
COMMENT ON COLUMN ground_items.item_id IS '物品ID';
COMMENT ON COLUMN ground_items.quantity IS '数量';
COMMENT ON COLUMN ground_items.dropped_by IS '掉落者Agent ID（可选）';

-- ============================================================================
-- 记录版本
-- ============================================================================
INSERT INTO schema_version (version, description)
VALUES (6, 'GroundItems: ground_items table for dropped items')
ON CONFLICT (version) DO NOTHING;
