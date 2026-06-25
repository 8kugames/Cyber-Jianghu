-- 019_agent_daily_summaries_fk.sql
-- 修复 P1-17：agent_daily_summaries 缺失 agents(agent_id) 的 ON DELETE CASCADE，
-- 导致父行被删后子行成为孤儿、chronicle 收集器默默吞下脏数据。
-- 兄弟表（004/007/010/018）已全部 CASCADE，本次补齐 schema 风格不变量。

ALTER TABLE agent_daily_summaries
    ADD CONSTRAINT fk_agent_daily_summaries_agent
    FOREIGN KEY (agent_id) REFERENCES agents(agent_id) ON DELETE CASCADE;

COMMENT ON CONSTRAINT fk_agent_daily_summaries_agent ON agent_daily_summaries
    IS 'P1-17 修复：agent 删除时级联清理日摘要，避免孤儿行';
