-- 020_device_token_rotation.sql
-- 修复 P1-12：devices.auth_token 一次生成终身有效，缺乏 TTL / rotation 机制。
-- 泄漏一次 = 永久失陷。
--
-- 本次只补物理结构 + 时间戳，不改任何运行时逻辑（避免一次 PR 引发回归）：
-- - `token_created_at`：token 签发时间（含轮换后重置）
-- - `token_rotated_at`：最近轮换时间（首次签发为 NULL）
--
-- 接入点（后续 PR）：
-- - `retire_agent` 成功末尾：调用 `rotate_device_token` 让旧凭据立即失效
-- - 显式 rotation 端点 / 调度器轮换
-- - TTL 阈值通过 config 注入（避免硬编码）

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'devices' AND column_name = 'token_created_at'
    ) THEN
        ALTER TABLE devices
            ADD COLUMN token_created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP;
    END IF;
END $$;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'devices' AND column_name = 'token_rotated_at'
    ) THEN
        ALTER TABLE devices
            ADD COLUMN token_rotated_at TIMESTAMPTZ;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_devices_token_created_at
    ON devices(token_created_at);

COMMENT ON COLUMN devices.token_created_at IS 'P1-12：当前 auth_token 签发/最近轮换时刻';
COMMENT ON COLUMN devices.token_rotated_at IS 'P1-12：最近 rotation 时刻；首次签发时为 NULL';
