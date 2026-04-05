-- ============================================================================
-- 002_devices.sql
-- ============================================================================
-- Device identity table (authentication). One device can create multiple
-- characters across rebirths.
-- ============================================================================

CREATE TABLE IF NOT EXISTS devices (
    device_id   UUID PRIMARY KEY,    -- client-generated UUID v4
    auth_token  TEXT NOT NULL UNIQUE, -- server-generated, used for API + WS auth
    last_seen   TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    created_at  TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_devices_auth_token ON devices(auth_token);
CREATE INDEX IF NOT EXISTS idx_devices_last_seen  ON devices(last_seen);

COMMENT ON TABLE devices IS '设备身份表 - 存储客户端设备的认证信息';
COMMENT ON COLUMN devices.device_id IS '设备唯一标识（客户端生成 UUID v4）';
COMMENT ON COLUMN devices.auth_token IS '认证令牌（服务器生成，用于 API 和 WebSocket 认证）';
COMMENT ON COLUMN devices.last_seen IS '设备最后在线时间';
COMMENT ON COLUMN devices.created_at IS '设备首次注册时间';
