-- ============================================================================
-- OpenClaw Cyber-Jianghu MVP Database Schema
-- Version: 1.0
-- Description: MVP阶段最小数据库schema，仅包含核心功能所需的表
-- ============================================================================

-- 启用UUID扩展
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ============================================================================
-- 1. Agent基本信息表
-- ============================================================================
CREATE TABLE agents (
    agent_id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name VARCHAR(100) NOT NULL,
    system_prompt TEXT NOT NULL,
    auth_token VARCHAR(255) UNIQUE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    last_tick_online TIMESTAMP WITH TIME ZONE
);

CREATE INDEX idx_agents_auth_token ON agents(auth_token);
CREATE INDEX idx_agents_last_online ON agents(last_tick_online);

COMMENT ON TABLE agents IS 'Agent基本信息表';
COMMENT ON COLUMN agents.agent_id IS 'Agent唯一ID';
COMMENT ON COLUMN agents.name IS 'Agent名称（如：老板娘、富商、刀客等）';
COMMENT ON COLUMN agents.system_prompt IS 'Agent人设Prompt（LLM使用）';
COMMENT ON COLUMN agents.auth_token IS '认证token（WebSocket连接时使用）';
COMMENT ON COLUMN agents.last_tick_online IS '最后一次上报意图的时间';

-- ============================================================================
-- 2. Agent状态表（每Tick更新）
-- ============================================================================
CREATE TABLE agent_states (
    id BIGSERIAL PRIMARY KEY,
    agent_id UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    tick_id BIGINT NOT NULL,
    hp INTEGER NOT NULL DEFAULT 100 CHECK (hp >= 0 AND hp <= 100),
    stamina INTEGER NOT NULL DEFAULT 100 CHECK (stamina >= 0 AND stamina <= 100),
    hunger INTEGER NOT NULL DEFAULT 100 CHECK (hunger >= 0 AND hunger <= 100),
    thirst INTEGER NOT NULL DEFAULT 100 CHECK (thirst >= 0 AND thirst <= 100),
    node_id VARCHAR(100) NOT NULL DEFAULT 'longmen_inn',
    is_alive BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,

    UNIQUE(agent_id, tick_id)
);

CREATE INDEX idx_agent_states_agent_id ON agent_states(agent_id);
CREATE INDEX idx_agent_states_tick_id ON agent_states(tick_id);
CREATE INDEX idx_agent_states_alive ON agent_states(is_alive);

COMMENT ON TABLE agent_states IS 'Agent状态表，每Tick记录一次状态快照';
COMMENT ON COLUMN agent_states.tick_id IS 'Tick编号（递增）';
COMMENT ON COLUMN agent_states.hp IS '生命值（0-100），归零死亡';
COMMENT ON COLUMN agent_states.stamina IS '体力值（0-100），用于执行动作和战斗';
COMMENT ON COLUMN agent_states.hunger IS '饥饿值（0-100），每小时衰减5点，归零死亡';
COMMENT ON COLUMN agent_states.thirst IS '口渴值（0-100），每小时衰减5点，归零死亡';
COMMENT ON COLUMN agent_states.node_id IS '当前所在节点ID（MVP阶段固定为"longmen_inn"）';
COMMENT ON COLUMN agent_states.is_alive IS '是否存活';

-- ============================================================================
-- 3. 物品模板表
-- ============================================================================
CREATE TABLE items (
    item_id VARCHAR(50) PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    item_type VARCHAR(50) NOT NULL CHECK (item_type IN ('consumable', 'weapon', 'currency')),
    effect_type VARCHAR(50),
    effect_value INTEGER DEFAULT 0,
    description TEXT
);

COMMENT ON TABLE items IS '物品模板表，定义所有物品的属性';
COMMENT ON COLUMN items.item_id IS '物品唯一ID（如：mantou, water, silver, knife）';
COMMENT ON COLUMN items.item_type IS '物品类型：consumable(消耗品)/weapon(武器)/currency(货币)';
COMMENT ON COLUMN items.effect_type IS '效果类型：restore_hunger/restore_thirst/increase_attack';
COMMENT ON COLUMN items.effect_value IS '效果值（如：馒头恢复饥饿值30点）';

-- ============================================================================
-- 4. Agent背包表（Agent拥有的物品）
-- ============================================================================
CREATE TABLE agent_inventory (
    id BIGSERIAL PRIMARY KEY,
    agent_id UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    item_id VARCHAR(50) NOT NULL REFERENCES items(item_id),
    quantity INTEGER NOT NULL DEFAULT 1 CHECK (quantity > 0),
    is_equipped BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,

    UNIQUE(agent_id, item_id)
);

CREATE INDEX idx_agent_inventory_agent_id ON agent_inventory(agent_id);
CREATE INDEX idx_agent_inventory_item_id ON agent_inventory(item_id);

COMMENT ON TABLE agent_inventory IS 'Agent背包表，记录Agent拥有的物品';
COMMENT ON COLUMN agent_inventory.quantity IS '物品数量（同类物品可堆叠，每格最多10个）';
COMMENT ON COLUMN agent_inventory.is_equipped IS '是否已装备（仅对武器有效）';

-- ============================================================================
-- 5. Tick日志表（记录每次Tick的执行信息）
-- ============================================================================
CREATE TABLE tick_logs (
    tick_id BIGSERIAL PRIMARY KEY,
    started_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at TIMESTAMP WITH TIME ZONE,
    duration_ms INTEGER,
    agents_processed INTEGER DEFAULT 0,
    actions_executed INTEGER DEFAULT 0,
    status VARCHAR(50) NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'completed', 'failed')),
    error_message TEXT
);

CREATE INDEX idx_tick_logs_started_at ON tick_logs(started_at);
CREATE INDEX idx_tick_logs_status ON tick_logs(status);

COMMENT ON TABLE tick_logs IS 'Tick日志表，记录每次Tick的执行情况';
COMMENT ON COLUMN tick_logs.tick_id IS 'Tick编号（自增）';
COMMENT ON COLUMN tick_logs.duration_ms IS 'Tick执行耗时（毫秒）';
COMMENT ON COLUMN tick_logs.agents_processed IS '处理的Agent数量';
COMMENT ON COLUMN tick_logs.actions_executed IS '执行的动作数量';
COMMENT ON COLUMN tick_logs.status IS 'Tick状态：running/completed/failed';

-- ============================================================================
-- 6. Agent动作日志表（记录Agent的所有动作）
-- ============================================================================
CREATE TABLE agent_action_logs (
    id BIGSERIAL PRIMARY KEY,
    tick_id BIGINT NOT NULL REFERENCES tick_logs(tick_id),
    agent_id UUID NOT NULL REFERENCES agents(agent_id),
    action_type VARCHAR(50) NOT NULL,
    action_data JSONB,
    result VARCHAR(50),
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_action_logs_tick_id ON agent_action_logs(tick_id);
CREATE INDEX idx_action_logs_agent_id ON agent_action_logs(agent_id);
CREATE INDEX idx_action_logs_action_type ON agent_action_logs(action_type);

COMMENT ON TABLE agent_action_logs IS 'Agent动作日志表，记录Agent执行的所有动作';
COMMENT ON COLUMN agent_action_logs.action_type IS '动作类型：idle/speak/give/steal/use/attack';
COMMENT ON COLUMN agent_action_logs.action_data IS '动作详细数据（JSON格式）';
COMMENT ON COLUMN agent_action_logs.result IS '动作执行结果：success/failed';

-- ============================================================================
-- 7. 初始化数据 - 物品模板
-- ============================================================================
INSERT INTO items (item_id, name, item_type, effect_type, effect_value, description) VALUES
('mantou', '馒头', 'consumable', 'restore_hunger', 30, '恢复饥饿值30点'),
('water', '水', 'consumable', 'restore_thirst', 30, '恢复口渴值30点'),
('silver', '银子', 'currency', NULL, 0, '货币，用于交易'),
('knife', '刀', 'weapon', 'increase_attack', 5, '武器，增加攻击力5点');

COMMENT ON TABLE items IS 'MVP阶段仅包含4种基础物品';

-- ============================================================================
-- 8. 性能优化 - 部分索引（仅索引存活Agent）
-- ============================================================================
CREATE INDEX idx_agent_states_alive_only ON agent_states(agent_id) WHERE is_alive = true;

-- ============================================================================
-- 9. 触发器 - 自动更新updated_at字段
-- ============================================================================
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ language 'plpgsql';

CREATE TRIGGER update_agent_inventory_updated_at
    BEFORE UPDATE ON agent_inventory
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- ============================================================================
-- Schema版本控制
-- ============================================================================
CREATE TABLE schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    description TEXT
);

INSERT INTO schema_version (version, description) VALUES (1, 'Initial MVP schema');

-- ============================================================================
-- 完成提示
-- ============================================================================
-- MVP阶段数据库schema创建完成
--
-- 包含的表：
-- 1. agents - Agent基本信息
-- 2. agent_states - Agent状态（HP、饥饿值、口渴值）
-- 3. items - 物品模板（馒头、水、银子、刀）
-- 4. agent_inventory - Agent背包
-- 5. tick_logs - Tick执行日志
-- 6. agent_action_logs - Agent动作日志
-- 7. schema_version - Schema版本控制
--
-- 未包含的功能（推迟到后续版本）：
-- - Agent关系表（好友、师徒）
-- - 交易历史表
-- - 经济统计表
-- - 多节点表
-- - 武学技能表
--
-- 注意事项：
-- 1. 所有表都包含必要的索引
-- 2. 使用了外键约束保证数据一致性
-- 3. 使用了CHECK约束保证数据有效性
-- 4. 包含详细的COMMENT说明
-- ============================================================================
