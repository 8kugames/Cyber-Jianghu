-- ============================================================================
-- 016_agent_daily_summaries.sql
-- ============================================================================
-- Agent 每日 LLM 日志摘要存档表
--
-- 每游戏日结束时由 SessionTriageEngine 生成，提交给 Server 存档。
-- 用途：
--   1. 永久存储每日摘要，支持 Agent 重生/换设备后历史追溯
--   2. Chronicle 聚合时 LEFT JOIN，注入 AgentSummary.narrative

CREATE TABLE IF NOT EXISTS agent_daily_summaries (
    id          BIGSERIAL PRIMARY KEY,
    agent_id    UUID NOT NULL,
    game_day    BIGINT NOT NULL,
    summary     TEXT NOT NULL,
    created_at  BIGINT NOT NULL,  -- Server Unix ms（服务器权威时间，非客户端）
    UNIQUE(agent_id, game_day)
);

CREATE INDEX IF NOT EXISTS idx_agent_daily_summaries_agent_game
    ON agent_daily_summaries(agent_id, game_day);
CREATE INDEX IF NOT EXISTS idx_agent_daily_summaries_game_day
    ON agent_daily_summaries(game_day);

COMMENT ON TABLE agent_daily_summaries IS 'Agent 每日 LLM 日志摘要存档';
COMMENT ON COLUMN agent_daily_summaries.summary IS 'produce_daily_summary 生成的格式化摘要';
COMMENT ON COLUMN agent_daily_summaries.created_at IS '服务器接收时的 Unix ms 时间戳';
