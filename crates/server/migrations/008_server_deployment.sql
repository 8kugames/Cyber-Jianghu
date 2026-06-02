-- ============================================================================
-- 008_server_deployment.sql
-- ============================================================================
-- server_deployment: 单行表，持久化服务器首次部署时间。
-- 解决"服务器运行时间"在每次重启后被重置的问题。
-- 设计：使用 CHECK (id = 1) 强制单行，ON CONFLICT DO NOTHING 保证幂等。
-- ============================================================================

CREATE TABLE IF NOT EXISTS server_deployment (
    id           SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    deployed_at  TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- 首次启动时插入一行；后续启动 ON CONFLICT 跳过，保留首次部署时间
INSERT INTO server_deployment (id, deployed_at)
VALUES (1, CURRENT_TIMESTAMP)
ON CONFLICT (id) DO NOTHING;

COMMENT ON TABLE  server_deployment        IS '服务器部署元信息（单行表）';
COMMENT ON COLUMN server_deployment.id      IS '固定为 1，CHECK 约束保证单行';
COMMENT ON COLUMN server_deployment.deployed_at IS '服务器首次部署时间（首次启动时写入，重启不变）';
