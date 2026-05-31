-- ============================================================================
-- 004_items.sql
-- ============================================================================
-- items           -- 物品模板（items.yaml 驱动）
-- agent_inventory -- 角色背包
-- ground_items    -- 地面掉落物
-- ============================================================================

CREATE TABLE IF NOT EXISTS items (
    item_id     VARCHAR(50) PRIMARY KEY,
    name        VARCHAR(100) NOT NULL,
    item_type   VARCHAR(50) NOT NULL,
    effects     JSONB DEFAULT '[]',
    stack_size  INTEGER DEFAULT 10,
    description TEXT
);

COMMENT ON TABLE  items IS '物品模板表（数据由 items.yaml 驱动）';
COMMENT ON COLUMN items.item_type IS '物品类型：consumable/weapon/currency/material/tool';

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

DROP TRIGGER IF EXISTS update_agent_inventory_updated_at ON agent_inventory;
CREATE TRIGGER update_agent_inventory_updated_at
    BEFORE UPDATE ON agent_inventory
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TABLE IF NOT EXISTS ground_items (
    id         BIGSERIAL PRIMARY KEY,
    node_id    VARCHAR(100) NOT NULL,
    item_id    VARCHAR(50) NOT NULL REFERENCES items(item_id),
    quantity   INTEGER NOT NULL DEFAULT 1 CHECK (quantity > 0),
    dropped_by UUID REFERENCES agents(agent_id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_ground_items_node_id ON ground_items(node_id);
CREATE INDEX IF NOT EXISTS idx_ground_items_item_id ON ground_items(item_id);
