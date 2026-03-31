-- ============================================================================
-- 003_agents.sql
-- ============================================================================
-- agents       -- identity + persona (status: active/retired/dead)
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

CREATE INDEX IF NOT EXISTS idx_agents_device_id       ON agents(device_id);
CREATE INDEX IF NOT EXISTS idx_agents_status           ON agents(status);
CREATE INDEX IF NOT EXISTS idx_agents_device_status    ON agents(device_id, status);
CREATE INDEX IF NOT EXISTS idx_agents_last_tick_online ON agents(last_tick_online);

COMMENT ON TABLE agents IS 'Agent基本信息表';
COMMENT ON COLUMN agents.agent_id IS 'Agent唯一ID';
COMMENT ON COLUMN agents.device_id IS '所属设备ID（外键关联 devices 表）';
COMMENT ON COLUMN agents.name IS 'Agent名称（如：老板娘、富商、刀客等）';
COMMENT ON COLUMN agents.system_prompt IS 'Agent人设Prompt（LLM使用）';
COMMENT ON COLUMN agents.status IS 'Agent状态：active=活跃, retired=归隐（主动）, dead=死亡（被动）';
COMMENT ON COLUMN agents.retired_at IS '归隐时间（转生时设置）';
COMMENT ON COLUMN agents.last_tick_online IS '最后一次上报意图的时间';

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

CREATE INDEX IF NOT EXISTS idx_agent_states_agent_id       ON agent_states(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_states_tick_id        ON agent_states(tick_id);
CREATE INDEX IF NOT EXISTS idx_agent_states_is_alive       ON agent_states(is_alive);
CREATE INDEX IF NOT EXISTS idx_agent_states_alive_only     ON agent_states(agent_id) WHERE is_alive = true;
CREATE INDEX IF NOT EXISTS idx_agent_states_attributes_gin ON agent_states USING GIN (attributes);

COMMENT ON TABLE agent_states IS 'Agent状态表，每Tick记录一次状态快照';
COMMENT ON COLUMN agent_states.tick_id IS 'Tick编号（递增）';
COMMENT ON COLUMN agent_states.attributes IS '动态属性（JSONB），完全数据驱动，支持任意扩展';
COMMENT ON COLUMN agent_states.node_id IS '当前所在节点ID';
COMMENT ON COLUMN agent_states.is_alive IS '是否存活';
