-- 014_add_stage_column.sql
-- 三皇共审管道阶段持久化
--
-- 管道流程：
--   阶段 1：伏羲初审（awaiting_fuxi_initial）→ 拒绝直接关单 / 批准进入阶段 2
--   阶段 2：神农 + 轩辕并行审议（awaiting_peer）→ 全部拒绝关单 / ≥1 票批准进入阶段 3
--   阶段 3：伏羲终审调整 + 写入（awaiting_fuxi_final）→ 写入 actions.yaml 后转 done
--
-- stage 取值：awaiting_fuxi_initial / awaiting_peer / awaiting_fuxi_final / done

ALTER TABLE action_evolution_proposal_groups
    ADD COLUMN IF NOT EXISTS stage TEXT NOT NULL DEFAULT 'awaiting_fuxi_initial';

COMMENT ON COLUMN action_evolution_proposal_groups.stage
    IS '管道阶段：awaiting_fuxi_initial / awaiting_peer / awaiting_fuxi_final / done';
