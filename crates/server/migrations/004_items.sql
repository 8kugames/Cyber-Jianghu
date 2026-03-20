-- ============================================================================
-- 004_items.sql - 物品系统
-- ============================================================================
--
-- 包含：
-- - items: 物品模板表（定义所有物品的属性）
-- - agent_inventory: Agent 背包表（Agent 拥有的物品）
--
-- 设计说明：
-- - items.effects 使用 JSONB 存储效果数组，匹配 items.yaml 配置
-- - 物品数据由配置文件驱动，数据库表仅用于 FK 约束
-- - 支持堆叠、装备等机制
-- ============================================================================

-- ============================================================================
-- items 表 - 物品模板
-- ============================================================================
CREATE TABLE items (
    -- 物品唯一 ID（如：mantou, water, silver, knife）
    item_id VARCHAR(50) PRIMARY KEY,

    -- 物品名称
    name VARCHAR(100) NOT NULL,

    -- 物品类型（consumable/weapon/currency/material/tool）
    item_type VARCHAR(50) NOT NULL,

    -- 物品效果数组（JSONB）
    -- 格式: [{"attribute": "hunger", "operation": "add", "value": 30}]
    effects JSONB DEFAULT '[]',

    -- 最大堆叠数量
    stack_size INTEGER DEFAULT 10,

    -- 物品描述
    description TEXT
);

-- 注释
COMMENT ON TABLE items IS '物品模板表，定义所有物品的属性（数据由 items.yaml 驱动）';
COMMENT ON COLUMN items.item_id IS '物品唯一ID（如：mantou, water, silver, knife）';
COMMENT ON COLUMN items.item_type IS '物品类型：consumable/weapon/currency/material/tool';
COMMENT ON COLUMN items.effects IS '物品效果数组 [{attribute, operation, value}]';
COMMENT ON COLUMN items.stack_size IS '最大堆叠数量';
COMMENT ON COLUMN items.description IS '物品描述';

-- ============================================================================
-- agent_inventory 表 - Agent 背包
-- ============================================================================
CREATE TABLE agent_inventory (
    -- 记录 ID
    id BIGSERIAL PRIMARY KEY,

    -- Agent ID（外键，级联删除）
    agent_id UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,

    -- 物品 ID（外键关联 items 表）
    item_id VARCHAR(50) NOT NULL REFERENCES items(item_id),

    -- 物品数量
    quantity INTEGER NOT NULL DEFAULT 1 CHECK (quantity > 0),

    -- 是否已装备（仅对武器有效）
    is_equipped BOOLEAN NOT NULL DEFAULT false,

    -- 当前耐久度（默认 -1 表示无限)
    durability INTEGER DEFAULT -1,

    -- 创建时间
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    -- 更新时间（由触发器自动维护）
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    -- 唯一约束：每个 Agent 每种物品只能有一条记录
    UNIQUE(agent_id, item_id)
);

-- 索引
CREATE INDEX idx_agent_inventory_agent_id ON agent_inventory(agent_id);
CREATE INDEX idx_agent_inventory_item_id ON agent_inventory(item_id);

-- 触发器：自动更新 updated_at
CREATE TRIGGER update_agent_inventory_updated_at
    BEFORE UPDATE ON agent_inventory
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- 注释
COMMENT ON TABLE agent_inventory IS 'Agent背包表，记录Agent拥有的物品';
COMMENT ON COLUMN agent_inventory.quantity IS '物品数量（同类物品可堆叠）';
COMMENT ON COLUMN agent_inventory.is_equipped IS '是否已装备（仅对武器有效）';
COMMENT ON COLUMN agent_inventory.updated_at IS '更新时间（由触发器自动维护）';

-- ============================================================================
-- 记录版本
-- ============================================================================
INSERT INTO schema_version (version, description)
VALUES (4, 'Items: items template and agent_inventory tables with JSONB effects')
ON CONFLICT (version) DO NOTHING;
