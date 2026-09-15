#!/usr/bin/env bash
# check-round.sh - 联调测试单轮健康快照（SKILL Phase 3 抽查/监测用，只观测不干预）
#
# 用法:
#   .test-agents/monitor-tools/check-round.sh [LOOKBACK_MIN]   # 日志回看窗口，默认 10 分钟
#
# 输出: 健康状态表（角色/Hunger/HP/Sanity/位置/Tick）+ token 统计 + 近 N 分钟错误日志
# 数据目录与 monitor-24h.sh 一致（.test-agents/agent-*/data/），token 源为 setup/status 权威值

set -u
cd "$(dirname "$0")/../.."

COMPOSE=".test-agents/docker-compose.yml"
INTERVAL_MIN=${1:-10}
ROUND_DIR="./tmp/check-round-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$ROUND_DIR"

# 端点发现：与 monitor-24h.sh 同一解析逻辑（service:container）
AGENTS=($(awk '
  BEGIN { in_svc=0; cur=""; cn="" }
  /^services:/ { in_svc=1; next }
  in_svc && /^  [a-z][a-z0-9_-]*:$/ {
    if (cur!="" && cn!="") print cur":"cn
    cur=$1; sub(/:$/,"",cur); cn=""; next }
  in_svc && /container_name:/ { cn=$2; next }
  END { if (cur!="" && cn!="") print cur":"cn }' "$COMPOSE"))

if [ ${#AGENTS[@]} -eq 0 ]; then
  echo "FATAL: $COMPOSE 中未发现 agent 服务" >&2
  exit 1
fi

# ── 并发采集：character + state + 容器状态 + 错误日志 ─────────────────────────
for entry in "${AGENTS[@]}"; do
  svc="${entry%%:*}"
  c="${entry##*:}"
  (
    if [ -s ".agent-static-token" ]; then
      token=$(cat ".agent-static-token")
    else
      token=$(docker exec "$c" curl -sf --max-time 5 \
        "http://127.0.0.1:23340/api/v1/setup/status" 2>/dev/null | \
        jq -r '.auth_token // empty' 2>/dev/null)
    fi
    docker exec "$c" curl -s --max-time 30 -H "Authorization: Bearer $token" \
      http://127.0.0.1:23340/api/v1/character > "$ROUND_DIR/$svc.char" 2>/dev/null
    docker exec "$c" curl -s --max-time 30 -H "Authorization: Bearer $token" \
      http://127.0.0.1:23340/api/v1/state > "$ROUND_DIR/$svc.state" 2>/dev/null
    docker ps --filter "name=$c" --format "{{.Status}}" > "$ROUND_DIR/$svc.docker" 2>/dev/null
    docker logs --since "${INTERVAL_MIN}m" "$c" 2>&1 | \
      grep -E "ERROR|WARN|死亡|death|panic" | tail -20 > "$ROUND_DIR/$svc.log" 2>/dev/null
    docker logs --since "${INTERVAL_MIN}m" "$c" 2>&1 | \
      grep -oE "决策: \S+" | sed 's/决策: //' > "$ROUND_DIR/$svc.actions" 2>/dev/null
    true
  ) &
done
wait

# ── 健康状态表 ────────────────────────────────────────────────────────────────
echo "| Agent | 容器 | 角色 | Hunger | HP | Sanity | 状态 | 位置 | Tick | 容器状态 |"
echo "| ----- | ---- | ---- | ------ | -- | ------ | ---- | ---- | ---- | -------- |"
for entry in "${AGENTS[@]}"; do
  svc="${entry%%:*}"
  c="${entry##*:}"
  docker_status="$(tr -d '\n' < "$ROUND_DIR/$svc.docker" 2>/dev/null)"
  [ -z "$docker_status" ] && docker_status="?"
  # 注意：jq 对空文件返回 exit 0 + 空输出，必须用 ${row:-} 兜底而非 if ! 检测
  row="$(jq -r --arg svc "$svc" --arg c "$c" --arg dk "$docker_status" '
      "| \($svc) | \($c) | \(.name // "-") | \(.attributes.hunger.current // "-") | \(.attributes.hp.current // "-") | \(.attributes.sanity.current // "-") | \(.status // "-") | \(.location // "-") | \(.tick_id // "-") | \($dk) |"' \
      "$ROUND_DIR/$svc.char" 2>/dev/null)"
  echo "${row:-| $svc | $c | - | - | - | - | NO_DATA | - | - | $docker_status |}"
done

# ── token 统计（累计值；看增量需对比上一轮）──────────────────────────────────
echo "--- token (累计值) ---"
for f in .test-agents/agent-*/data/logs/token_cost_count.tmp; do
  [ -f "$f" ] || continue
  svc=$(echo "$f" | cut -d/ -f2)
  out="$(jq -r --arg svc "$svc" '.summary.by_provider_model | to_entries[] |
    "\($svc) | \(.key) | calls=\(.value.total_calls // "?") prompt=\(.value.total_prompt_tokens // "?") completion=\(.value.total_completion_tokens // "?") failures=\(.value.total_failures // "?")"' \
    "$f" 2>/dev/null)"
  echo "${out:-$svc | parse_error}"
done | sort

# ── 行为分布（决策动作 + 香农熵，行为坏缩早期信号）─────────────────────────
echo "--- action distribution (last ${INTERVAL_MIN}m) ---"
echo "| Agent | 总决策 | 熵 H/Hmax | r | 判读 | 分布 |"
echo "| ----- | ------ | --------- | - | ---- | ---- |"
for entry in "${AGENTS[@]}"; do
  svc="${entry%%:*}"
  if [ ! -s "$ROUND_DIR/$svc.actions" ]; then
    echo "| $svc | 0 | - | - | 无决策 | - |"
    continue
  fi
  # 香农熵 H = -sum(p*log2(p))；r = H/Hmax（k=1 时 Hmax 取 1.0，与旧 python 版一致）
  jq -rRs --arg svc "$svc" '
    split("\n") | map(select(length > 0)) as $acts
    | ($acts | group_by(.) | map({a: .[0], n: length}) | sort_by(-.n)) as $dist
    | ($dist | map(.n) | add // 0) as $total
    | if $total == 0 then
        "| \($svc) | 0 | - | - | 无决策 | - |"
      else
        ($dist | map(.n as $n | ($n / $total) as $p | - ($p * ($p | log2))) | add) as $h
        | (if ($dist | length) > 1 then (($dist | length) | log2) else 1.0 end) as $hmax
        | ($h / $hmax) as $r
        | (if $r > 0.6 then "健康" elif $r >= 0.3 then "收缩" else "坍缩疑似" end) as $band
        | "| \($svc) | \($total) | \($h * 100 | round / 100)/\($hmax * 100 | round / 100) | \($r * 100 | round / 100) | \($band) | \($dist | map("\(.a):\(.n)") | join(",")) |"
      end
  ' "$ROUND_DIR/$svc.actions" 2>/dev/null || echo "| $svc | 0 | - | - | 无决策 | - |"
done
# 保存每 agent 动作计数（Phase 4 跨轮次聚合用）
for entry in "${AGENTS[@]}"; do
  svc=$(echo "$entry" | cut -d: -f1)
  [ -s "$ROUND_DIR/$svc.actions" ] && sort "$ROUND_DIR/$svc.actions" | uniq -c | awk -v svc="$svc" '{print svc "," $2 "," $1}' >> "$ROUND_DIR/actions.csv"
done

# ── 错误日志 ────────────────────────────────────────────────────────────────
echo "--- errors/warnings (last ${INTERVAL_MIN}m) ---"
found=0
for f in "$ROUND_DIR"/*.log; do
  [ -s "$f" ] || continue
  found=1
  echo "[$(basename "$f" .log)]"
  head -8 "$f"
done
[ "$found" = "0" ] && echo "(clean: 近 ${INTERVAL_MIN} 分钟无 ERROR/WARN/死亡/panic)"
echo "round dir: $ROUND_DIR"
