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

ALTER TABLE soul_review_votes
    ADD CONSTRAINT soul_review_votes_group_soul_role_unique
    UNIQUE (proposal_group_id, soul, role);
