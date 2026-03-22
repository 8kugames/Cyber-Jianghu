-- ============================================================================
-- 001_infrastructure.sql - 基础设施
-- ============================================================================
--
-- 包含：
-- - PostgreSQL 扩展
-- - Schema 版本控制表
-- - 通用触发器函数
--
-- 此文件必须首先执行
-- ============================================================================

-- ============================================================================
-- 1. PostgreSQL 扩展
-- ============================================================================
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ============================================================================
-- 2. Schema 版本控制表
-- ============================================================================
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY,
    applied_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
    description TEXT
);

COMMENT ON TABLE schema_version IS 'Schema版本控制表，记录已应用的迁移';

-- ============================================================================
-- 3. 通用触发器函数
-- ============================================================================
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ language 'plpgsql';

COMMENT ON FUNCTION update_updated_at_column() IS '通用触发器函数：自动更新 updated_at 字段';

-- ============================================================================
-- 记录版本
-- ============================================================================
INSERT INTO schema_version (version, description)
VALUES (1, 'Infrastructure: uuid-ossp extension, schema_version table, trigger functions')
ON CONFLICT (version) DO NOTHING;
