-- ============================================================================
-- 003_agents.sql
-- ============================================================================
-- agents       -- identity + persona (status includes active/retired/dead)
-- agent_states -- per-tick attribute snapshots (JSONB-driven)
-- ============================================================================

-- ---------------------------------------------------------------------------
-- agents
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS agents (
    agent_id       UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    device_id      UUID NOT NULL REFERENCES devices(device_id),
    name           VARCHAR(100) NOT NULL,
    system_prompt  TEXT NOT NULL,
    status         VARCHAR(20) NOT NULL DEFAULT 'active'
                       CHECK (status IN ('active', 'retired', 'dead')),
    retired_at     TIMESTAMPTZ,
    created_at     TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    last_tick_online TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_agents_device_id      ON agents(device_id);
CREATE INDEX IF NOT EXISTS idx_agents_status          ON agents(status);
CREATE INDEX IF NOT EXISTS idx_agents_device_status   ON agents(device_id, status);
CREATE INDEX IF NOT EXISTS idx_agents_last_tick_online ON agents(last_tick_online);

-- ---------------------------------------------------------------------------
-- agent_states
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS agent_states (
    id         BIGSERIAL PRIMARY KEY,
    agent_id   UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    tick_id    BIGINT NOT NULL,
    attributes JSONB NOT NULL DEFAULT '{}',
    node_id    VARCHAR(100) NOT NULL DEFAULT 'longmen_inn',
    is_alive   BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    UNIQUE(agent_id, tick_id)
);

CREATE INDEX IF NOT EXISTS idx_agent_states_agent_id    ON agent_states(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_states_tick_id     ON agent_states(tick_id);
CREATE INDEX IF NOT EXISTS idx_agent_states_is_alive    ON agent_states(is_alive);
CREATE INDEX IF NOT EXISTS idx_agent_states_alive_only  ON agent_states(agent_id) WHERE is_alive = true;
CREATE INDEX IF NOT EXISTS idx_agent_states_attributes_gin ON agent_states USING GIN (attributes);
