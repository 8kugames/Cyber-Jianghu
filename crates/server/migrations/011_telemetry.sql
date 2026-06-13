-- ============================================================================
-- 011_telemetry.sql
-- ============================================================================
-- telemetry_aggregations -- 行为遥测聚合表
--
-- 所有遥测类型的通用聚合表（1 张表 + JSONB metrics + GIN index）
-- aggregation_name 对应 telemetry_config.yaml 中的 name
-- group_by_key / group_by_value 是维度字段
-- metrics 是 JSONB 指标列
--
-- 设计理由：
-- - 5 种聚合的 schema 只有 group_by 字段和 metrics 字段不同
-- - 1 张表 + JSONB 比 N 张硬编码表更符合数据驱动原则
-- - 新增聚合类型不需要 migration（只在 telemetry_config.yaml 中加配置即可）
-- - PostgreSQL JSONB 支持 GIN 索引
-- ============================================================================

CREATE TABLE IF NOT EXISTS telemetry_aggregations (
    id              BIGSERIAL PRIMARY KEY,
    aggregation_name TEXT NOT NULL,                    -- survival_time / decision_distribution / ...
    period_start    TIMESTAMPTZ NOT NULL,
    period_end      TIMESTAMPTZ NOT NULL,
    group_by_key    TEXT,                              -- "action_type" / "result" / "node_id"
    group_by_value  TEXT,                              -- 该维度的具体值
    metrics         JSONB NOT NULL,                    -- {count: 42, avg_duration: 3600, ...}
    created_at      TIMESTAMPTZ DEFAULT NOW()
);

-- 按聚合类型 + 时间范围过滤（主要查询模式）
CREATE INDEX idx_telemetry_agg_name_period
    ON telemetry_aggregations(aggregation_name, period_start DESC);

-- JSONB GIN 索引（支持指标值过滤）
CREATE INDEX idx_telemetry_agg_gin
    ON telemetry_aggregations USING GIN (metrics);

COMMENT ON TABLE  telemetry_aggregations IS '行为遥测聚合数据';
COMMENT ON COLUMN telemetry_aggregations.aggregation_name IS '聚合类型名称（对应 YAML 中 name）';
COMMENT ON COLUMN telemetry_aggregations.period_start IS '聚合周期开始时间';
COMMENT ON COLUMN telemetry_aggregations.period_end IS '聚合周期结束时间';
COMMENT ON COLUMN telemetry_aggregations.group_by_key IS '分组维度 key';
COMMENT ON COLUMN telemetry_aggregations.group_by_value IS '分组维度值';
COMMENT ON COLUMN telemetry_aggregations.metrics IS '指标数据（JSONB）';
