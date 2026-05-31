-- ============================================================================
-- 005_tick_system.sql
-- ============================================================================
-- tick_logs          -- tick 执行记录
-- agent_action_logs  -- 角色 action 日志（Pipeline 展平：每 pipe_seq 一条）
-- ============================================================================

-- tick_logs
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

COMMENT ON TABLE  tick_logs IS 'Tick日志表';
COMMENT ON COLUMN tick_logs.tick_id IS 'Tick编号（由代码提供）';
COMMENT ON COLUMN tick_logs.status IS 'Tick状态：running/completed/failed';

-- agent_action_logs
CREATE TABLE IF NOT EXISTS agent_action_logs (
    id                   BIGSERIAL PRIMARY KEY,
    tick_id              BIGINT NOT NULL,
    agent_id             UUID NOT NULL REFERENCES agents(agent_id),
    pipe_seq             INTEGER NOT NULL DEFAULT 0,
    action_type          VARCHAR(50) NOT NULL,
    action_type_display  VARCHAR(200),
    action_data          JSONB,
    result               VARCHAR(50),
    result_message       TEXT,
    thought_log          TEXT,
    reflector_thought    TEXT,
    narrative            TEXT,
    soul_cycle_metadata  JSONB,
    chaos_marker         JSONB,
    dream_marker         JSONB,
    created_at           TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_agent_action_logs_tick_id      ON agent_action_logs(tick_id);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_agent_id     ON agent_action_logs(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_action_type  ON agent_action_logs(action_type);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_result_tick  ON agent_action_logs(result, tick_id DESC);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_soul_cycle   ON agent_action_logs(agent_id, tick_id DESC)
    WHERE soul_cycle_metadata IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_action_logs_agent_tick_pipe_unique
    ON agent_action_logs(agent_id, tick_id, pipe_seq);

COMMENT ON TABLE  agent_action_logs IS 'Agent动作日志（Pipeline 展平：每 pipe_seq 一条）';
COMMENT ON COLUMN agent_action_logs.tick_id             IS 'Tick 编号（由墙钟计算）';
COMMENT ON COLUMN agent_action_logs.pipe_seq            IS 'Pipeline 内序号（0=primary）';
COMMENT ON COLUMN agent_action_logs.action_type         IS '动作类型';
COMMENT ON COLUMN agent_action_logs.action_type_display IS '动作中文描述';
COMMENT ON COLUMN agent_action_logs.action_data         IS '动作详细数据（JSONB）';
COMMENT ON COLUMN agent_action_logs.result              IS '执行结果：success/failed';
COMMENT ON COLUMN agent_action_logs.result_message      IS '执行结果详细描述';
COMMENT ON COLUMN agent_action_logs.thought_log         IS 'ActorSoul 思考日志';
COMMENT ON COLUMN agent_action_logs.reflector_thought   IS 'ReflectorSoul 审查理由';
COMMENT ON COLUMN agent_action_logs.narrative           IS '叙事化经历描述';
COMMENT ON COLUMN agent_action_logs.soul_cycle_metadata IS '三魂循环完整元数据 JSONB';
COMMENT ON COLUMN agent_action_logs.chaos_marker        IS '混沌行为标记 JSONB';
COMMENT ON COLUMN agent_action_logs.dream_marker        IS '托梦影响标记 JSONB';
