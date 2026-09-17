-- 026_resource_nodes.sql
-- 资源点存量模型（阶段 2）：持久存量 + 日再生，见 docs/features/resource_depletion.md
--
-- 采集在 Saga 事务内扣减（stock >= quantity 守卫，不足即枯竭失败并整体回滚）；
-- 日再生由 scheduler 日历日递增触发点按 regen_per_game_day 回补（受四季
-- resource_growth_rate 加成，上限 max_stock）。
CREATE TABLE IF NOT EXISTS resource_nodes (
    node_id             TEXT NOT NULL,
    item_id             TEXT NOT NULL,
    stock               BIGINT NOT NULL,
    max_stock           BIGINT NOT NULL,
    regen_per_game_day  BIGINT NOT NULL DEFAULT 0,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (node_id, item_id)
);

CREATE INDEX IF NOT EXISTS idx_resource_nodes_node_id ON resource_nodes(node_id);
