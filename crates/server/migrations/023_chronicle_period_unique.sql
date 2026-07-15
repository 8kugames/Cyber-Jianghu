-- 023_chronicle_period_unique.sql
-- C4：chronicles 幂等写入前置 —— 为 (period_start, period_end) 建唯一约束
--
-- 背景：chronicle storage.rs 的 INSERT 现在要加 ON CONFLICT DO UPDATE，前提是
-- (period_start, period_end) 上有 UNIQUE 约束作为冲突仲裁键。否则 ON CONFLICT
-- 编译期就要报 "there is no unique or exclusion constraint matching the ON CONFLICT"。
--
-- 安全性：
-- 1. 建约束前先清理潜在的重复行（保留每组 period_start,period_end 中 id 最大的一条，
--    删除其余）。生产环境若无重复，DELETE 0 行，无副作用。
-- 2. 用 DO $$ ... IF NOT EXISTS 包裹建约束，保证迁移幂等可重跑。
-- 3. 约束名固定为 uq_chronicles_period_start_period_end，便于后续 IF NOT EXISTS 判定。

-- ============================================================================
-- 步骤 1：清理重复行（同 period_start, period_end 只保留最新 id）
-- ============================================================================
DELETE FROM chronicles
WHERE id NOT IN (
    SELECT MAX(id)
    FROM chronicles
    GROUP BY period_start, period_end
);

-- ============================================================================
-- 步骤 2：幂等加 UNIQUE 约束
-- ============================================================================
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'uq_chronicles_period_start_period_end'
          AND conrelid = 'chronicles'::regclass
    ) THEN
        ALTER TABLE chronicles
            ADD CONSTRAINT uq_chronicles_period_start_period_end
            UNIQUE (period_start, period_end);
    END IF;
END
$$;

COMMENT ON CONSTRAINT uq_chronicles_period_start_period_end ON chronicles
    IS 'C4: 幂等写入冲突仲裁键（同周期只允许一条 chronicle）';
