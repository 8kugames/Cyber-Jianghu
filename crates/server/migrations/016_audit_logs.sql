-- ============================================================================
-- 016_audit_logs.sql
-- ============================================================================
-- P0-16: 为关键管理操作提供可检索、可追责的审计日志
-- ============================================================================

CREATE TABLE IF NOT EXISTS audit_logs (
    id            BIGSERIAL PRIMARY KEY,
    event_type    VARCHAR(100) NOT NULL,
    actor_type    VARCHAR(50) NOT NULL,
    token_type    VARCHAR(20),
    resource_type VARCHAR(100) NOT NULL,
    resource_id   TEXT,
    endpoint      TEXT NOT NULL,
    method        VARCHAR(16) NOT NULL,
    result        VARCHAR(20) NOT NULL,
    reason        TEXT,
    payload       JSONB NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_audit_logs_created_at    ON audit_logs(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_logs_event_type    ON audit_logs(event_type);
CREATE INDEX IF NOT EXISTS idx_audit_logs_resource_type ON audit_logs(resource_type);

COMMENT ON TABLE audit_logs IS '关键管理操作审计日志';
COMMENT ON COLUMN audit_logs.result IS '操作结果：success/failure/denied';
