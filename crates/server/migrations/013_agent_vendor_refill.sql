-- Vendor 自动补货规则表
-- 任何 Agent 都可配置为 Vendor，通过 Admin UI 管理补货规则
CREATE TABLE IF NOT EXISTS agent_vendor_refill (
    agent_id UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    item_id TEXT NOT NULL,
    -- 库存低于此值触发补货
    threshold INT NOT NULL DEFAULT 10,
    -- 单次最大补货量
    refill_to INT NOT NULL DEFAULT 50,
    -- 最大消耗银两百分比 (0-100)
    budget_ratio INT NOT NULL DEFAULT 50,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_id, item_id)
);

CREATE INDEX IF NOT EXISTS idx_vendor_refill_enabled ON agent_vendor_refill(enabled) WHERE enabled = true;
