#!/bin/bash
# ============================================================================
# Docker Container Entrypoint for Cyber-Jianghu Agent
# 在容器启动前检查并修复权限问题
# ============================================================================

set -e

# 容器内运行用户的 UID/GID
TARGET_UID=${CONTAINER_UID:-1000}
TARGET_GID=${CONTAINER_GID:-1000}
TARGET_USER=${CONTAINER_USER:-cyberjianghu}

# 以 root 用户运行时，检查并修复目录权限
if [ "$(id -u)" = "0" ]; then
    # 检查 data 目录是否可写
    if [ -d "/app/data" ]; then
        CURRENT_OWNER=$(stat -c '%u:%g' /app/data 2>/dev/null || stat -f '%u:%g' /app/data)
        if [ "$CURRENT_OWNER" != "$TARGET_UID:$TARGET_GID" ]; then
            echo "[INFO] 修复 /app/data 目录权限: $CURRENT_OWNER -> $TARGET_UID:$TARGET_GID"
            chown -R "$TARGET_UID:$TARGET_GID" /app/data
        fi
    fi

    # 检查 config 目录是否可写
    if [ -d "/app/config" ]; then
        CURRENT_OWNER=$(stat -c '%u:%g' /app/config 2>/dev/null || stat -f '%u:%g' /app/config)
        if [ "$CURRENT_OWNER" != "$TARGET_UID:$TARGET_GID" ]; then
            echo "[INFO] 修复 /app/config 目录权限: $CURRENT_OWNER -> $TARGET_UID:$TARGET_GID"
            chown -R "$TARGET_UID:$TARGET_GID" /app/config
        fi
    fi

    # 切换到非 root 用户执行命令
    echo "[INFO] 切换到用户 $TARGET_USER (UID=$TARGET_UID) 运行服务"
    exec runuser -u "$TARGET_USER" -- "$@"
else
    # 已经是非 root 用户，直接执行
    exec "$@"
fi
