-- Agent 角色身份绑定表（运行时追加）
-- 与 initial_recipes.yaml roles 段对应，支持 Admin 面板动态授予角色身份
CREATE TABLE IF NOT EXISTS agent_assigned_roles (
    agent_id UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    role_key TEXT NOT NULL,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (agent_id, role_key)
);

CREATE INDEX IF NOT EXISTS idx_agent_assigned_roles_agent ON agent_assigned_roles(agent_id);

COMMENT ON TABLE agent_assigned_roles IS 'Agent 角色身份绑定（运行时追加，对应 initial_recipes.yaml roles）';
COMMENT ON COLUMN agent_assigned_roles.role_key IS '角色标识，对应 initial_recipes.yaml 中的 role_key';
