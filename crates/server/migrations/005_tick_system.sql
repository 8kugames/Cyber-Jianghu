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

COMMENT ON TABLE tick_logs IS 'Tick日志表，记录每次Tick的执行情况';
COMMENT ON COLUMN tick_logs.tick_id IS 'Tick编号（由代码提供，非自增）';
COMMENT ON COLUMN tick_logs.duration_ms IS 'Tick执行耗时（毫秒）';
COMMENT ON COLUMN tick_logs.agents_processed IS '处理的Agent数量';
COMMENT ON COLUMN tick_logs.actions_executed IS '执行的动作数量';
COMMENT ON COLUMN tick_logs.status IS 'Tick状态：running/completed/failed';

-- ---------------------------------------------------------------------------
-- agent_action_logs
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS agent_action_logs (
    id               BIGSERIAL PRIMARY KEY,
    tick_id          BIGINT NOT NULL REFERENCES tick_logs(tick_id),
    agent_id         UUID NOT NULL REFERENCES agents(agent_id),
    action_type      VARCHAR(50) NOT NULL,
    action_data      JSONB,
    result           VARCHAR(50),
    thought_log      TEXT,
    observer_thought TEXT,
    narrative        TEXT,
    created_at       TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_agent_action_logs_tick_id     ON agent_action_logs(tick_id);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_agent_id    ON agent_action_logs(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_action_type ON agent_action_logs(action_type);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_narrative   ON agent_action_logs(agent_id, tick_id DESC);

COMMENT ON TABLE agent_action_logs IS 'Agent动作日志表，记录Agent执行的所有动作';
COMMENT ON COLUMN agent_action_logs.action_type IS '动作类型：idle/speak/give/steal/use/attack/move/gather/craft';
COMMENT ON COLUMN agent_action_logs.action_data IS '动作详细数据（JSON格式）';
COMMENT ON COLUMN agent_action_logs.result IS '动作执行结果：success/failed';
COMMENT ON COLUMN agent_action_logs.thought_log IS 'ActorSoul 思考日志';
COMMENT ON COLUMN agent_action_logs.observer_thought IS 'ReflectorSoul 审查理由';
COMMENT ON COLUMN agent_action_logs.narrative IS '叙事化经历描述';
