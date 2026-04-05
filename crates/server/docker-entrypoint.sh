#!/bin/bash
# ============================================================================
# Docker Container Entrypoint
# 权限修复、数据库迁移、启动服务
# ============================================================================

set -euo pipefail

TARGET_UID=${CONTAINER_UID:-1000}
TARGET_GID=${CONTAINER_GID:-1000}
TARGET_USER=${CONTAINER_USER:-cyberjianghu}
MIGRATION_DIR="/app/migrations"

# ---------------------------------------------------------------------------
# 等待数据库就绪
# ---------------------------------------------------------------------------
wait_for_db() {
    echo "[INFO] 等待数据库就绪..."
    local max_attempts=30
    local attempt=0
    while [ $attempt -lt $max_attempts ]; do
        if pg_isready -h "${POSTGRES_HOST:-postgres}" -p "${POSTGRES_PORT:-5432}" -U "${POSTGRES_USER:-postgres}" >/dev/null 2>&1; then
            echo "[INFO] 数据库已就绪"
            return 0
        fi
        attempt=$((attempt + 1))
        echo "[INFO] 等待数据库... ($attempt/$max_attempts)"
        sleep 2
    done
    echo "[ERROR] 数据库连接超时"
    return 1
}

# ---------------------------------------------------------------------------
# 执行数据库迁移（严格模式：任何错误立即终止）
# ---------------------------------------------------------------------------
run_migrations() {
    echo "[INFO] 执行数据库迁移..."

    if [ ! -d "$MIGRATION_DIR" ]; then
        echo "[ERROR] 迁移目录不存在: $MIGRATION_DIR"
        return 1
    fi

    for sql_file in "$MIGRATION_DIR"/*.sql; do
        [ -f "$sql_file" ] || continue
        local filename
        filename=$(basename "$sql_file")
        echo "[INFO] 执行迁移: $filename"

        # ON_ERROR_STOP=1: 遇到错误立即中止，不做静默吞错
        # --single-transaction: 整个文件包在事务里，失败自动回滚
        if ! psql "${DATABASE_URL}" -v ON_ERROR_STOP=1 --single-transaction -f "$sql_file" 2>&1; then
            echo "[ERROR] 迁移失败: $filename"
            return 1
        fi

        echo "[INFO] 迁移完成: $filename"
    done

    echo "[INFO] 数据库迁移完成"
}

# ---------------------------------------------------------------------------
# 主逻辑
# ---------------------------------------------------------------------------

# 以 root 用户运行时：修复权限、迁移数据库、切换到非 root 用户
if [ "$(id -u)" = "0" ]; then
    # 配置目录初始化：如果 /app/config 为空（volume 挂载了空目录），自动填充默认配置
    if [ -d "/app/default_config" ] && [ -d "/app/config" ]; then
        if [ -z "$(ls -A /app/config 2>/dev/null)" ]; then
            echo "[INFO] /app/config 为空，从默认配置填充..."
            cp -rT /app/default_config /app/config
            echo "[INFO] 默认配置已填充到 /app/config"
        fi
    fi

    # 修复 logs 目录权限
    if [ -d "/app/logs" ]; then
        CURRENT_OWNER=$(stat -c '%u:%g' /app/logs 2>/dev/null || stat -f '%u:%g' /app/logs)
        if [ "$CURRENT_OWNER" != "$TARGET_UID:$TARGET_GID" ]; then
            echo "[INFO] 修复 /app/logs 目录权限: $CURRENT_OWNER -> $TARGET_UID:$TARGET_GID"
            chown -R "$TARGET_UID:$TARGET_GID" /app/logs
        fi
    fi

    # 执行数据库迁移
    if [ -n "${DATABASE_URL:-}" ]; then
        wait_for_db
        run_migrations
    fi

    echo "[INFO] 切换到用户 $TARGET_USER (UID=$TARGET_UID) 运行服务"
    exec runuser -u "$TARGET_USER" -- "$@"
else
    # 非 root 用户：需要迁移时执行，否则显式提示
    if [ -n "${DATABASE_URL:-}" ]; then
        if command -v psql >/dev/null 2>&1; then
            wait_for_db
            run_migrations
        else
            echo "[WARN] DATABASE_URL 已设置但 psql 未找到，跳过迁移"
        fi
    fi
    exec "$@"
fi
