#!/usr/bin/env bash
# MVP 验收 run 观测快照脚本
#
# 定时抓取 server 的 健康度看板与涌现检测端点，落盘 JSON 快照。
# 只读观测，不改变任何 server 状态（验收纪律：只观测不干预）。
#
# 用法：
#   ADMIN_READ_TOKEN=xxx ./scripts/acceptance_snapshot.sh            # 每小时一次，前台运行
#   INTERVAL_SECS=600 ADMIN_READ_TOKEN=xxx ./scripts/acceptance_snapshot.sh
#
# 环境变量：
#   SERVER_URL        默认 http://localhost:23333
#   ADMIN_READ_TOKEN  必填（或 CLIENT_READ_TOKEN，dashboard read 端点两者皆收）
#   INTERVAL_SECS     快照间隔，默认 3600（验收纪律：每小时）
#   OUT_DIR           快照目录，默认 ./tmp/acceptance_logs
#
# 快照文件：snap-YYYYMMDD-HHMMSS.json（health + emergence 合并）。
# run 结束后整目录随 DB dump 一起归档。

set -eu

SERVER_URL="${SERVER_URL:-http://localhost:23333}"
INTERVAL_SECS="${INTERVAL_SECS:-3600}"
OUT_DIR="${OUT_DIR:-./tmp/acceptance_logs}"
WINDOW=240  # MVP 观测窗口

if [ -z "${ADMIN_READ_TOKEN:-}" ]; then
    echo "[错误] 未设置 ADMIN_READ_TOKEN（dashboard read 端点需要 Bearer token）" >&2
    exit 1
fi

mkdir -p "$OUT_DIR"

# 记录 run 基线（一次性）
BASELINE="$OUT_DIR/baseline.md"
if [ ! -f "$BASELINE" ]; then
    {
        echo "COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo '?')"
        echo "START=$(date)"
        echo "SERVER_URL=$SERVER_URL"
        echo "INTERVAL_SECS=$INTERVAL_SECS"
        echo "WINDOW=$WINDOW"
    } > "$BASELINE"
    echo "[基线] $BASELINE"
fi

snapshot_once() {
    local ts now out
    now=$(date '+%Y%m%d-%H%M%S')
    out="$OUT_DIR/snap-$now.json"
    ts=$(date -u '+%Y-%m-%dT%H:%M:%SZ')

    local health emergence
    health=$(curl -sf -H "Authorization: Bearer $ADMIN_READ_TOKEN" \
        "$SERVER_URL/api/dashboard/health?window=$WINDOW" || echo '{"error":"fetch_failed"}')
    emergence=$(curl -sf -H "Authorization: Bearer $ADMIN_READ_TOKEN" \
        "$SERVER_URL/api/dashboard/emergence?window=$WINDOW" || echo '{"error":"fetch_failed"}')

    # 合并为单个 JSON 对象；jq 不可用时退化为分开落盘
    if command -v jq >/dev/null 2>&1; then
        jq -n --arg ts "$ts" --argjson h "$health" --argjson e "$emergence" \
            '{snapshot_utc: $ts, health: $h, emergence: $e}' > "$out" 2>/dev/null \
            || { echo "{\"snapshot_utc\":\"$ts\",\"health\":$health,\"emergence\":$emergence}" > "$out"; }
    else
        echo "{\"snapshot_utc\":\"$ts\",\"health\":$health,\"emergence\":$emergence}" > "$out"
    fi
    echo "[快照] $out"
}

echo "[开始] 每 ${INTERVAL_SECS}s 抓取一次，窗口 window=$WINDOW，输出 $OUT_DIR（Ctrl-C 结束）"
while true; do
    snapshot_once
    sleep "$INTERVAL_SECS"
done
