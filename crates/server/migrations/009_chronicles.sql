-- ============================================================================
-- 009_chronicles.sql
-- ============================================================================
-- 群像传记表：每 7 游戏日生成一份世界群像
-- ============================================================================

-- 群像传记主表
CREATE TABLE IF NOT EXISTS chronicles (
    id              BIGSERIAL PRIMARY KEY,
    chronicle_id    VARCHAR(20) UNIQUE NOT NULL,  -- 如 "C-001"
    period_start    BIGINT NOT NULL,               -- 开始 tick_id
    period_end      BIGINT NOT NULL,               -- 结束 tick_id
    game_day_start  INT NOT NULL,                  -- 开始游戏日
    game_day_end    INT NOT NULL,                  -- 结束游戏日
    season          VARCHAR(50) NOT NULL,           -- 季节名称
    summary         TEXT NOT NULL,                 -- 总览（模板生成）
    summary_llm     TEXT,                          -- LLM 增强版本
    agent_count     INT NOT NULL DEFAULT 0,        -- 参与 agent 数量
    actions_count   INT NOT NULL DEFAULT 0,        -- 总动作数
    highlights      JSONB,                          -- 关键事件列表
    agent_summaries JSONB,                         -- 每个 agent 的简报
    action_stats    JSONB,                         -- 动作类型统计
    location_stats  JSONB,                         -- 地点分布统计
    deaths          INT NOT NULL DEFAULT 0,        -- 死亡人数
    births          INT NOT NULL DEFAULT 0,        -- 新注册人数
    raw_data        JSONB,                          -- 原始聚合数据
    status          VARCHAR(20) NOT NULL DEFAULT 'template',  -- template / llm / both
    created_at      TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_chronicles_period ON chronicles(period_start, period_end);
CREATE INDEX IF NOT EXISTS idx_chronicles_game_day ON chronicles(game_day_start DESC);
CREATE INDEX IF NOT EXISTS idx_chronicles_created_at ON chronicles(created_at DESC);

COMMENT ON TABLE chronicles IS '群像传记表，每7游戏日生成一份世界群像';
COMMENT ON COLUMN chronicles.chronicle_id IS '传记ID，如 C-001';
COMMENT ON COLUMN chronicles.summary IS '模板生成的总览';
COMMENT ON COLUMN chronicles.summary_llm IS 'LLM 增强版本';
COMMENT ON COLUMN chronicles.status IS '生成状态：template / llm / both';
