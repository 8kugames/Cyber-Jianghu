-- ============================================================================
-- 005_tick_system.sql - Tick 日志系统
-- ============================================================================
--
-- 包含：
-- - tick_logs: Tick 执行日志（记录每次 Tick 的执行情况）
-- - agent_action_logs: Agent 动作日志（记录 Agent 执行的所有动作）
--
-- 设计说明：
-- - tick_logs 是主表，agent_action_logs 通过 tick_id 关联
-- - 用于调试、统计和回放
-- ============================================================================

-- ============================================================================
-- tick_logs 表 - Tick 执行日志
-- ============================================================================
CREATE TABLE IF NOT EXISTS tick_logs (
    -- Tick 编号（由代码提供，非自增）
    tick_id BIGINT PRIMARY KEY,

    -- 开始时间
    started_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,

    -- 完成时间
    completed_at TIMESTAMPTZ,

    -- 执行耗时（毫秒）
    duration_ms INTEGER,

    -- 处理的 Agent 数量
    agents_processed INTEGER DEFAULT 0,

    -- 执行的动作数量
    actions_executed INTEGER DEFAULT 0,

    -- Tick 状态
    status VARCHAR(50) NOT NULL DEFAULT 'running'
        CHECK (status IN ('running', 'completed', 'failed')),

    -- 错误信息（如果失败）
    error_message TEXT
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_tick_logs_started_at ON tick_logs(started_at);
CREATE INDEX IF NOT EXISTS idx_tick_logs_status ON tick_logs(status);

-- 注释
COMMENT ON TABLE tick_logs IS 'Tick日志表，记录每次Tick的执行情况';
COMMENT ON COLUMN tick_logs.tick_id IS 'Tick编号（自增）';
COMMENT ON COLUMN tick_logs.duration_ms IS 'Tick执行耗时（毫秒）';
COMMENT ON COLUMN tick_logs.agents_processed IS '处理的Agent数量';
COMMENT ON COLUMN tick_logs.actions_executed IS '执行的动作数量';
COMMENT ON COLUMN tick_logs.status IS 'Tick状态：running/completed/failed';

-- ============================================================================
-- agent_action_logs 表 - Agent 动作日志
-- ============================================================================
CREATE TABLE IF NOT EXISTS agent_action_logs (
    -- 记录 ID
    id BIGSERIAL PRIMARY KEY,

    -- Tick 编号（外键关联 tick_logs）
    tick_id BIGINT NOT NULL REFERENCES tick_logs(tick_id),

    -- Agent ID（外键关联 agents）
    agent_id UUID NOT NULL REFERENCES agents(agent_id),

    -- 动作类型
    action_type VARCHAR(50) NOT NULL,

    -- 动作详细数据（JSONB）
    action_data JSONB,

    -- 执行结果
    result VARCHAR(50),

    -- 创建时间
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_tick_id ON agent_action_logs(tick_id);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_agent_id ON agent_action_logs(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_action_logs_action_type ON agent_action_logs(action_type);

-- 注释
COMMENT ON TABLE agent_action_logs IS 'Agent动作日志表，记录Agent执行的所有动作';
COMMENT ON COLUMN agent_action_logs.action_type IS '动作类型：idle/speak/give/steal/use/attack/move/gather/craft';
COMMENT ON COLUMN agent_action_logs.action_data IS '动作详细数据（JSON格式）';
COMMENT ON COLUMN agent_action_logs.result IS '动作执行结果：success/failed';

-- ============================================================================
-- 记录版本
-- ============================================================================
INSERT INTO schema_version (version, description)
VALUES (5, 'TickSystem: tick_logs (BIGINT tick_id) and agent_action_logs tables')
ON CONFLICT (version) DO NOTHING;
