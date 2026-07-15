-- 022_agent_relationships.sql
-- C1 阶段：服务端关系图谱存储
--
-- 全量快照同步策略：每个游戏日结束时 agent 把完整关系快照上报，
-- server 全量覆盖（DELETE+INSERT），天然幂等。复刻 DailySummary 同步链路。
--
-- 表语义镜像 agent 端 crates/agent/src/component/social/relationship_types.rs：
--   RelationshipMemory + KeyEvent（单向：source → target）
-- 时间戳使用 BIGINT Unix 毫秒（与 protocol 对齐，避免 DateTime 进 DB）。

-- ============================================================================
-- agent_relationships：source 对 target 的单向关系
-- ============================================================================
CREATE TABLE IF NOT EXISTS agent_relationships (
    -- 关系持有者（A 对 B 的看法）
    source_agent_id   UUID        NOT NULL,
    -- 关系目标
    target_agent_id   UUID        NOT NULL,
    target_name       TEXT        NOT NULL,
    -- 好感度 -100..100，0 为中性
    favorability      INTEGER     NOT NULL CHECK (favorability >= -100 AND favorability <= 100),
    last_interaction_tick BIGINT   NOT NULL,
    -- 快照写入时间（Unix 毫秒，server 权威）
    synced_at         BIGINT      NOT NULL,
    self_description  TEXT        NOT NULL,
    description_tick  BIGINT      NOT NULL,
    PRIMARY KEY (source_agent_id, target_agent_id)
);

-- ============================================================================
-- agent_relationship_key_events：关系的关键事件（子表，CASCADE）
-- ============================================================================
CREATE TABLE IF NOT EXISTS agent_relationship_key_events (
    id                BIGSERIAL   PRIMARY KEY,
    source_agent_id   UUID        NOT NULL,
    target_agent_id   UUID        NOT NULL,
    tick_id           BIGINT      NOT NULL,
    event_type        TEXT        NOT NULL,
    description       TEXT        NOT NULL,
    favorability_delta INTEGER    NOT NULL,
    -- 事件时间戳（Unix 毫秒）
    event_timestamp   BIGINT      NOT NULL,
    FOREIGN KEY (source_agent_id, target_agent_id)
        REFERENCES agent_relationships (source_agent_id, target_agent_id)
        ON DELETE CASCADE
);

-- ============================================================================
-- 索引（幂等）
-- ============================================================================
CREATE INDEX IF NOT EXISTS idx_agent_relationship_key_events_src_tgt
    ON agent_relationship_key_events (source_agent_id, target_agent_id);

CREATE INDEX IF NOT EXISTS idx_agent_relationship_key_events_tick
    ON agent_relationship_key_events (tick_id DESC);

CREATE INDEX IF NOT EXISTS idx_agent_relationships_source
    ON agent_relationships (source_agent_id);

-- ============================================================================
-- 注释
-- ============================================================================
COMMENT ON TABLE agent_relationships IS 'C1: agent 对目标的单向关系快照（全量同步）';
COMMENT ON TABLE agent_relationship_key_events IS 'C1: 关系关键事件（CASCADE 跟随 agent_relationships）';
