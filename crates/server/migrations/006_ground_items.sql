-- ============================================================================
-- 006_ground_items.sql
-- ============================================================================
-- ground_items -- items dropped on the ground in a location node
-- ============================================================================

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
