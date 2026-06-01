#!/usr/bin/env bash
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

fail() { echo -e "${RED}❌ $1${NC}"; }
ok() { echo -e "${GREEN}✅ $1${NC}"; }
warn() { echo -e "${YELLOW}⚠️  $1${NC}"; }

SERVER_URL="${SERVER_URL:-http://localhost:23333}"
ADMIN_TOKEN="${ADMIN_TOKEN:-}"
AGENT_ID="${AGENT_ID:-}"

if [ -z "$ADMIN_TOKEN" ]; then
  read -p "Admin token (从 dashboard 登录态 localStorage admin_token 复制): " ADMIN_TOKEN
fi
if [ -z "$AGENT_ID" ]; then
  read -p "Agent ID (任意一个角色,UUID 格式): " AGENT_ID
fi

echo
echo "=== 1. 检查 server 版本 ==="
VERSION=$(curl -sS "${SERVER_URL}/health" 2>/dev/null | head -1 || echo "unreachable")
if echo "$VERSION" | grep -q "0.1.155\|0.1.15[6-9]\|0.1.1[6-9]"; then
  ok "Server 已是修复版本 (>= v0.1.155)"
else
  fail "Server 仍是旧版本（响应: $VERSION）— 必须重启 server 才能加载新 binary"
  echo "       重启命令："
  echo "         docker: docker compose restart cyber-jianghu-server"
  echo "         local:  ./install.sh all restart  或  pkill cyber-jianghu-server && cargo run -p cyber-jianghu-server --release"
  echo "       然后重跑本脚本验证。"
  exit 1
fi

echo
echo "=== 2. 拉取一个 agent 的最新经历 ==="
RESP=$(curl -sS -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  "${SERVER_URL}/api/dashboard/agent/${AGENT_ID}/experiences?limit=3")
echo "$RESP" | head -c 500
echo

if echo "$RESP" | grep -q '"formatted_time"'; then
  ok "API 返回包含 formatted_time 字段（server 端格式化已生效）"
else
  fail "API 响应中找不到 formatted_time 字段 — server 没跑新 binary"
  exit 1
fi

if echo "$RESP" | grep -q '"game_day"'; then
  ok "API 返回包含 game_day 字段"
else
  warn "API 响应中找不到 game_day 字段（旧数据无 soul_cycle_metadata 可忽略）"
fi

echo
echo "=== 3. 检查 formatted_time 内容（不能含 JSON 字符串） ==="
FT_VALS=$(echo "$RESP" | grep -oE '"formatted_time":"[^"]+"' | head -3)
if echo "$FT_VALS" | grep -q '"formatted_time":"\\{"'; then
  fail "formatted_time 字段值仍是转义 JSON 字符串 — 数据本身有误，需检查 agent 端 world_time 上报"
else
  ok "formatted_time 字段值是格式化后的中文（不是 raw JSON）"
  echo "  示例："
  echo "$FT_VALS" | sed 's/^/    /'
fi

echo
echo "=== 4. 浏览器侧验证 ==="
echo "  在 admin dashboard 打开角色详情（点角色 → 经历日志 tab）"
echo "  期望看到时间列显示形如 '二七四年元月四日子时'，不是"
echo "    {\"year\":274,...} ← 旧 binary 漏出"
echo "    [时间格式异常]    ← fallback 链全失败（极少见，需查 console）"
echo
echo "  如果还是漏出 raw JSON："
echo "    1. 浏览器 Ctrl+Shift+R 强制刷新（绕过缓存）"
echo "    2. DevTools → Network → Disable cache 勾选"
echo "    3. 确认三个 JS 文件已更新（utils.js / agents.js / history.js）"
echo
ok "全部检查通过。修复应在 dashboard 正确显示天道历时间。"
