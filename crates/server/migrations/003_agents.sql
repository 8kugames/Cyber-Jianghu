-- ============================================================================
-- 003_agents.sql - Agent 核心系统
-- ============================================================================
--
-- 包含：
-- - agents: Agent 基本信息（身份、人设）
-- - agent_states: Agent 状态快照（每 Tick 记录）
--
-- 设计说明：
-- - agent_states.attributes 使用 JSONB 实现完全数据驱动
-- - 支持任意属性扩展，无需修改表结构
-- ============================================================================

-- ============================================================================
-- agents 表 - Agent 基本信息
-- ============================================================================
CREATE TABLE IF NOT EXISTS agents (
    -- Agent 唯一 ID
    agent_id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),

    -- 所属设备（外键关联 devices 表）
    device_id UUID NOT NULL REFERENCES devices(device_id),

    -- Agent 名称（如：老板娘、富商、刀客等）
    name VARCHAR(100) NOT NULL,

    -- Agent 人设 Prompt（LLM 使用）
    system_prompt TEXT NOT NULL,

    -- 创建时间
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    -- 最后一次上报意图的时间
    last_tick_online TIMESTAMPTZ
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_agents_device_id ON agents(device_id);
CREATE INDEX IF NOT EXISTS idx_agents_last_tick_online ON agents(last_tick_online);

-- 注释
COMMENT ON TABLE agents IS 'Agent基本信息表';
COMMENT ON COLUMN agents.agent_id IS 'Agent唯一ID';
COMMENT ON COLUMN agents.device_id IS '所属设备ID（外键关联 devices 表）';
COMMENT ON COLUMN agents.name IS 'Agent名称（如：老板娘、富商、刀客等）';
COMMENT ON COLUMN agents.system_prompt IS 'Agent人设Prompt（LLM使用）';
COMMENT ON COLUMN agents.last_tick_online IS '最后一次上报意图的时间';

-- ============================================================================
-- agent_states 表 - Agent 状态快照
-- ============================================================================
CREATE TABLE IF NOT EXISTS agent_states (
    -- 记录 ID
    id BIGSERIAL PRIMARY KEY,

    -- Agent ID（外键，级联删除）
    agent_id UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,

    -- Tick 编号
    tick_id BIGINT NOT NULL,

    -- 动态属性（JSONB，完全数据驱动）
    attributes JSONB NOT NULL DEFAULT '{}',

    -- 当前所在节点 ID
    node_id VARCHAR(100) NOT NULL DEFAULT 'longmen_inn',

    -- 是否存活
    is_alive BOOLEAN NOT NULL DEFAULT true,

    -- 创建时间
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    -- 唯一约束：每个 Agent 每个 Tick 只能有一条记录
    UNIQUE(agent_id, tick_id)
);

-- 基础索引
CREATE INDEX IF NOT EXISTS idx_agent_states_agent_id ON agent_states(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_states_tick_id ON agent_states(tick_id);
CREATE INDEX IF NOT EXISTS idx_agent_states_is_alive ON agent_states(is_alive);

-- 部分索引: 仅索引存活的 Agent
CREATE INDEX IF NOT EXISTS idx_agent_states_alive_only ON agent_states(agent_id) WHERE is_alive = true;

-- JSONB GIN 索引: 支持高效属性查询
CREATE INDEX IF NOT EXISTS idx_agent_states_attributes_gin ON agent_states USING GIN (attributes);

-- 注释
COMMENT ON TABLE agent_states IS 'Agent状态表，每Tick记录一次状态快照';
COMMENT ON COLUMN agent_states.tick_id IS 'Tick编号（递增）';
COMMENT ON COLUMN agent_states.attributes IS '动态属性（JSONB），完全数据驱动，支持任意扩展';
COMMENT ON COLUMN agent_states.node_id IS '当前所在节点ID';
COMMENT ON COLUMN agent_states.is_alive IS '是否存活';

-- ============================================================================
-- 记录版本
-- ============================================================================
INSERT INTO schema_version (version, description)
VALUES (3, 'Agents: agents and agent_states tables with JSONB attributes')
ON CONFLICT (version) DO NOTHING;
