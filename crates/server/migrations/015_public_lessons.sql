-- 跨 Agent 传承 Layer 2: 共享教训库
--
-- 死亡事件按 cause 聚合，达到阈值后自动生成教训条目。
-- 教训通过 WorldState 下发给所有 Agent，供认知引擎参考。

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
