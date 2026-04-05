-- ============================================================================
-- 004_items.sql
-- ============================================================================
-- items           -- item templates (driven by items.yaml config)
-- agent_inventory -- per-agent owned items
-- ============================================================================

-- ---------------------------------------------------------------------------
-- items (template)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS items (
    item_id     VARCHAR(50) PRIMARY KEY,
    name        VARCHAR(100) NOT NULL,
    item_type   VARCHAR(50) NOT NULL,
    effects     JSONB DEFAULT '[]',
    stack_size  INTEGER DEFAULT 10,
    description TEXT
);

COMMENT ON TABLE items IS '物品模板表，定义所有物品的属性（数据由 items.yaml 驱动）';
COMMENT ON COLUMN items.item_id IS '物品唯一ID（如：mantou, water, silver, knife）';
COMMENT ON COLUMN items.item_type IS '物品类型：consumable/weapon/currency/material/tool';
COMMENT ON COLUMN items.effects IS '物品效果数组 [{attribute, operation, value}]';
COMMENT ON COLUMN items.stack_size IS '最大堆叠数量';
COMMENT ON COLUMN items.description IS '物品描述';

-- ---------------------------------------------------------------------------
-- agent_inventory
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS agent_inventory (
    id          BIGSERIAL PRIMARY KEY,
    agent_id    UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    item_id     VARCHAR(50) NOT NULL REFERENCES items(item_id),
    quantity    INTEGER NOT NULL DEFAULT 1 CHECK (quantity > 0),
    is_equipped BOOLEAN NOT NULL DEFAULT false,
    durability  INTEGER DEFAULT -1,
    created_at  TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at  TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    UNIQUE(agent_id, item_id)
);

CREATE INDEX IF NOT EXISTS idx_agent_inventory_agent_id ON agent_inventory(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_inventory_item_id  ON agent_inventory(item_id);

-- Auto-update updated_at
DROP TRIGGER IF EXISTS update_agent_inventory_updated_at ON agent_inventory;
CREATE TRIGGER update_agent_inventory_updated_at
    BEFORE UPDATE ON agent_inventory
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

COMMENT ON TABLE agent_inventory IS 'Agent背包表，记录Agent拥有的物品';
COMMENT ON COLUMN agent_inventory.quantity IS '物品数量（同类物品可堆叠）';
COMMENT ON COLUMN agent_inventory.is_equipped IS '是否已装备（仅对武器有效）';
COMMENT ON COLUMN agent_inventory.durability IS '当前耐久度（默认 -1 表示无限）';
COMMENT ON COLUMN agent_inventory.updated_at IS '更新时间（由触发器自动维护）';
