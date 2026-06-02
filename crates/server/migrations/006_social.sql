-- ============================================================================
-- 006_social.sql
-- ============================================================================
-- 群像传记、共享教训库、每日摘要
-- ============================================================================

-- chronicles（合并原 009）
CREATE TABLE IF NOT EXISTS chronicles (
    id              BIGSERIAL PRIMARY KEY,
    chronicle_id    VARCHAR(20) UNIQUE NOT NULL,
    period_start    BIGINT NOT NULL,
    period_end      BIGINT NOT NULL,
    game_day_start  INT NOT NULL,
    game_day_end    INT NOT NULL,
    season          VARCHAR(50) NOT NULL,
    summary         TEXT NOT NULL,
    summary_llm     TEXT,
    agent_count     INT NOT NULL DEFAULT 0,
    actions_count   INT NOT NULL DEFAULT 0,
    highlights      JSONB,
    agent_summaries JSONB,
    action_stats    JSONB,
    location_stats  JSONB,
    deaths          INT NOT NULL DEFAULT 0,
    births          INT NOT NULL DEFAULT 0,
    raw_data        JSONB,
    status          VARCHAR(20) NOT NULL DEFAULT 'template',
    created_at      TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_chronicles_period ON chronicles(period_start, period_end);
CREATE INDEX IF NOT EXISTS idx_chronicles_game_day ON chronicles(game_day_start DESC);
CREATE INDEX IF NOT EXISTS idx_chronicles_created_at ON chronicles(created_at DESC);

COMMENT ON TABLE chronicles IS '群像传记表，每7游戏日生成一份世界群像';

-- public_lessons（合并原 015）
CREATE TABLE IF NOT EXISTS public_lessons (
    id SERIAL PRIMARY KEY,
    cause TEXT NOT NULL UNIQUE,
    lesson TEXT NOT NULL,
    death_count INTEGER NOT NULL DEFAULT 1,
    avg_survival_ticks BIGINT,
    first_seen_tick BIGINT NOT NULL,
    last_seen_tick BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- agent_daily_summaries（合并原 016）
CREATE TABLE IF NOT EXISTS agent_daily_summaries (
    id          BIGSERIAL PRIMARY KEY,
    agent_id    UUID NOT NULL,
    game_day    BIGINT NOT NULL,
    summary     TEXT NOT NULL,
    created_at  BIGINT NOT NULL,
    UNIQUE(agent_id, game_day)
);

CREATE INDEX IF NOT EXISTS idx_agent_daily_summaries_agent_game
    ON agent_daily_summaries(agent_id, game_day);
CREATE INDEX IF NOT EXISTS idx_agent_daily_summaries_game_day
    ON agent_daily_summaries(game_day);
