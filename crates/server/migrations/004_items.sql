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
