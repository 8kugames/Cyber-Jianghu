-- soul_review_votes 唯一约束修正
--
-- 三阶段审议管道中伏羲以不同 role 对同一提案组投两票：
--   engine.rs 初审  persist_vote(..., role="primary")
--   engine.rs 终审  persist_vote(..., role="primary_final")
-- 原 UNIQUE(proposal_group_id, soul) 使终审票 INSERT 必然违反约束
-- （persist_vote 吞错只 warn），真正决定 actions.yaml 内容的终审裁决
-- 永不落库。约束改为 (proposal_group_id, soul, role)，同一灵魂在
-- 不同审议阶段的票各自独立；write_vote 同步改为 upsert，
-- 阶段重跑时新票覆盖旧票而非报错。

ALTER TABLE soul_review_votes
    DROP CONSTRAINT IF EXISTS soul_review_votes_proposal_group_id_soul_key;

-- 幂等建约束（与 023 同约定）：entrypoint 与 Rust run_migrations 每次启动
-- 都会重放全部迁移文件，裸 ADD CONSTRAINT 在二次执行时报
-- "relation ... already exists" 使 server 永久崩溃循环
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'soul_review_votes_group_soul_role_unique'
          AND conrelid = 'soul_review_votes'::regclass
    ) THEN
        ALTER TABLE soul_review_votes
            ADD CONSTRAINT soul_review_votes_group_soul_role_unique
            UNIQUE (proposal_group_id, soul, role);
    END IF;
END
$$;
