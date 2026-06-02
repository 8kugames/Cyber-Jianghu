-- ============================================================================
-- 002_devices.sql
-- ============================================================================
-- 设备身份表（认证）。一个设备可创建多个角色（跨转世）。
-- ============================================================================

CREATE TABLE IF NOT EXISTS devices (
    device_id   UUID PRIMARY KEY,
    auth_token  TEXT NOT NULL UNIQUE,
    last_seen   TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    created_at  TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_devices_auth_token ON devices(auth_token);
CREATE INDEX IF NOT EXISTS idx_devices_last_seen  ON devices(last_seen);

COMMENT ON TABLE  devices IS '设备身份表';
COMMENT ON COLUMN devices.device_id IS '设备唯一标识（客户端生成 UUID v4）';
COMMENT ON COLUMN devices.auth_token IS '认证令牌（服务器生成）';
