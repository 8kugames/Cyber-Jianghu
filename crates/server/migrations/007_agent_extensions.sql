-- ============================================================================
-- 007_agent_extensions.sql
-- ============================================================================
-- 角色扩展表：Vendor 补货、配方知识、观察学习、角色绑定
-- ============================================================================

-- agent_vendor_refill（合并原 013）
CREATE TABLE IF NOT EXISTS agent_vendor_refill (
    agent_id UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    item_id TEXT NOT NULL,
    threshold INT NOT NULL DEFAULT 10,
    refill_to INT NOT NULL DEFAULT 50,
    budget_ratio INT NOT NULL DEFAULT 50,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_id, item_id)
);

CREATE INDEX IF NOT EXISTS idx_vendor_refill_enabled ON agent_vendor_refill(enabled) WHERE enabled = true;

-- agent_known_recipes（合并原 021）
CREATE TABLE IF NOT EXISTS agent_known_recipes (
    agent_id  UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    recipe_id TEXT NOT NULL,
    learned_at_tick BIGINT NOT NULL DEFAULT 0,
    source    TEXT NOT NULL DEFAULT 'initial',
    source_detail JSONB DEFAULT '{}',
    PRIMARY KEY (agent_id, recipe_id)
);

CREATE INDEX IF NOT EXISTS idx_agent_known_recipes_agent ON agent_known_recipes(agent_id);

-- agent_recipe_observations（合并原 021）
CREATE TABLE IF NOT EXISTS agent_recipe_observations (
    agent_id       UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    recipe_id      TEXT NOT NULL,
    observation_count INT NOT NULL DEFAULT 1,
    last_seen_tick BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (agent_id, recipe_id)
);

-- agent_assigned_roles（合并原 022）
CREATE TABLE IF NOT EXISTS agent_assigned_roles (
    agent_id UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    role_key TEXT NOT NULL,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_id, role_key)
);

CREATE INDEX IF NOT EXISTS idx_agent_assigned_roles_agent ON agent_assigned_roles(agent_id);
