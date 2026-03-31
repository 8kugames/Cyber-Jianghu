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
