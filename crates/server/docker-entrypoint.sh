#!/bin/bash
# ============================================================================
# Docker Container Entrypoint
# 在容器启动前检查并修复权限问题，执行数据库迁移
# ============================================================================

set -e

# 容器内运行用户的 UID/GID
TARGET_UID=${CONTAINER_UID:-1000}
TARGET_GID=${CONTAINER_GID:-1000}
TARGET_USER=${CONTAINER_USER:-cyberjianghu}

# 等待数据库就绪
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

# 执行数据库迁移
run_migrations() {
    echo "[INFO] 执行数据库迁移..."
    local migration_dir="/app/migrations"
    
    if [ -d "$migration_dir" ]; then
        # 按文件名顺序执行迁移
        for sql_file in $(ls "$migration_dir"/*.sql 2>/dev/null | sort); do
            filename=$(basename "$sql_file")
            echo "[INFO] 执行迁移: $filename"
            if psql "${DATABASE_URL}" -f "$sql_file" >/dev/null 2>&1; then
                echo "[INFO] 迁移完成: $filename"
            else
                # 迁移可能已执行过，忽略错误
                echo "[INFO] 迁移跳过（可能已执行）: $filename"
            fi
        done
        echo "[INFO] 数据库迁移完成"
    else
        echo "[WARN] 迁移目录不存在: $migration_dir"
    fi
}

# 以 root 用户运行时，检查并修复权限问题
if [ "$(id -u)" = "0" ]; then
    # 检查 logs 目录是否可写
    if [ -d "/app/logs" ]; then
        CURRENT_OWNER=$(stat -c '%u:%g' /app/logs 2>/dev/null || stat -f '%u:%g' /app/logs)
        if [ "$CURRENT_OWNER" != "$TARGET_UID:$TARGET_GID" ]; then
            echo "[INFO] 修复 /app/logs 目录权限: $CURRENT_OWNER -> $TARGET_UID:$TARGET_GID"
            chown -R "$TARGET_UID:$TARGET_GID" /app/logs
        fi
    fi

    # 执行数据库迁移（在切换用户前）
    if [ -n "${DATABASE_URL}" ]; then
        wait_for_db && run_migrations
    fi

    # 切换到非 root 用户执行命令
    echo "[INFO] 切换到用户 $TARGET_USER (UID=$TARGET_UID) 运行服务"
    exec runuser -u "$TARGET_USER" -- "$@"
else
    # 已经是非 root 用户，直接执行
    exec "$@"
fi
