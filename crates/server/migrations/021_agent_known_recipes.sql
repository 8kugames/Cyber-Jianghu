-- 021_agent_known_recipes.sql
-- Agent 配方知识表 + 观察学习计数表

CREATE TABLE IF NOT EXISTS agent_known_recipes (
    agent_id  UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    recipe_id TEXT NOT NULL,
    learned_at_tick BIGINT NOT NULL DEFAULT 0,
    source    TEXT NOT NULL DEFAULT 'initial',
    source_detail JSONB DEFAULT '{}',
    PRIMARY KEY (agent_id, recipe_id)
);

CREATE INDEX IF NOT EXISTS idx_agent_known_recipes_agent ON agent_known_recipes(agent_id);

COMMENT ON TABLE agent_known_recipes IS 'Agent 已知配方表（Server 权威）';
COMMENT ON COLUMN agent_known_recipes.source IS '学习来源: initial / taught / observed';
COMMENT ON COLUMN agent_known_recipes.source_detail IS '来源详情 JSON: {teacher_id, observation_count, etc.}';

-- 观察学习计数表
CREATE TABLE IF NOT EXISTS agent_recipe_observations (
    agent_id       UUID NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    recipe_id      TEXT NOT NULL,
    observation_count INT NOT NULL DEFAULT 1,
    last_seen_tick BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (agent_id, recipe_id)
);
