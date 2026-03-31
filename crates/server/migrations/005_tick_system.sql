-- ============================================================================
-- 005_tick_system.sql
-- ============================================================================
-- tick_logs          -- tick execution records
-- agent_action_logs  -- per-agent action records (includes narrative fields)
-- ============================================================================

-- ---------------------------------------------------------------------------
-- tick_logs
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tick_logs (
    tick_id          BIGINT PRIMARY KEY,
    started_at       TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at     TIMESTAMPTZ,
    duration_ms      INTEGER,
    agents_processed INTEGER DEFAULT 0,
    actions_executed INTEGER DEFAULT 0,
    status           VARCHAR(50) NOT NULL DEFAULT 'running'
                         CHECK (status IN ('running', 'completed', 'failed')),
    error_message    TEXT
);

CREATE INDEX IF NOT EXISTS idx_tick_logs_started_at ON tick_logs(started_at);
CREATE INDEX IF NOT EXISTS idx_tick_logs_status     ON tick_logs(status);

-- ---------------------------------------------------------------------------
-- agent_action_logs
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS agent_action_logs (
    id              BIGSERIAL PRIMARY KEY,
    tick_id         BIGINT NOT NULL REFERENCES tick_logs(tick_id),
    agent_id        UUID NOT NULL REFERENCES agents(agent_id),
    action_type     VARCHAR(50) NOT NULL,
    action_data     JSONB,
    result          VARCHAR(50),
    thought_log     TEXT,
    observer_thought TEXT,
    narrative       TEXT,
    created_at      TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_agent_action_logs_tick_id     ON agent_action_logs(tick_id);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_agent_id    ON agent_action_logs(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_action_type ON agent_action_logs(action_type);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_narrative   ON agent_action_logs(agent_id, tick_id DESC);
