-- ============================================================================
-- 002_auth.sql - 设备认证系统
-- ============================================================================
--
-- 包含：
-- - devices: 设备身份表（认证令牌）
--
-- 设计说明：
-- - 设备身份与角色身份分离
-- - 一个设备可创建多个角色（转世机制）
-- - auth_token 用于 API 和 WebSocket 认证
-- ============================================================================

-- ============================================================================
-- devices 表
-- ============================================================================
CREATE TABLE IF NOT EXISTS devices (
    -- 设备唯一标识（客户端生成的 UUID v4）
    device_id UUID PRIMARY KEY,

    -- 认证令牌（服务器生成，用于后续所有 API 调用和 WebSocket 连接）
    auth_token TEXT NOT NULL UNIQUE,

    -- 最后在线时间
    last_seen TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    -- 创建时间
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

-- 索引
CREATE INDEX IF NOT EXISTS idx_devices_auth_token ON devices(auth_token);
CREATE INDEX IF NOT EXISTS idx_devices_last_seen ON devices(last_seen);

-- 注释
COMMENT ON TABLE devices IS '设备身份表 - 存储客户端设备的认证信息';
COMMENT ON COLUMN devices.device_id IS '设备唯一标识（客户端生成 UUID v4）';
COMMENT ON COLUMN devices.auth_token IS '认证令牌（服务器生成，用于 API 和 WebSocket 认证）';
COMMENT ON COLUMN devices.last_seen IS '设备最后在线时间';
COMMENT ON COLUMN devices.created_at IS '设备首次注册时间';

-- ============================================================================
-- 记录版本
-- ============================================================================
INSERT INTO schema_version (version, description)
VALUES (2, 'Auth: devices table for device authentication')
ON CONFLICT (version) DO NOTHING;
