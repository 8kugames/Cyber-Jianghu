-- ============================================================================
-- 017_audit_logs_context.sql
-- ============================================================================
-- P0-16 follow-up: 为审计日志补充请求上下文和前后状态
-- ============================================================================

ALTER TABLE audit_logs
    ADD COLUMN IF NOT EXISTS request_id TEXT,
    ADD COLUMN IF NOT EXISTS ip TEXT,
    ADD COLUMN IF NOT EXISTS user_agent TEXT,
    ADD COLUMN IF NOT EXISTS before_state JSONB,
    ADD COLUMN IF NOT EXISTS after_state JSONB;

CREATE INDEX IF NOT EXISTS idx_audit_logs_request_id ON audit_logs(request_id);
