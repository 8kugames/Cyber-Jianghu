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
        python3 -c "import json,sys; print(json.load(sys.stdin).get('auth_token',''))" 2>/dev/null)
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
python3 - "$ROUND_DIR" "${AGENTS[@]}" <<'PYEOF'
import json, os, sys
base, agents = sys.argv[1], sys.argv[2:]
print("| Agent | 角色 | Hunger | HP | Sanity | 状态 | 位置 | Tick | 容器 |")
print("| ----- | ---- | ------ | -- | ------ | ---- | ---- | ---- | ---- |")
for entry in agents:
    svc, c = entry.split(":", 1)
    name = hunger = hp = sanity = loc = tick = "-"
    status = "NO_DATA"
    docker = "?"
    try:
        docker = open(f"{base}/{svc}.docker").read().strip()
    except Exception:
        pass
    try:
        d = json.load(open(f"{base}/{svc}.char"))
        name = d.get("name", "-")
        status = d.get("status", "-")
        a = d.get("attributes", {})
        hunger = a.get("hunger", {}).get("current", "-")
        hp = a.get("hp", {}).get("current", "-")
        sanity = a.get("sanity", {}).get("current", "-")
        loc = d.get("location", "-")
        tick = d.get("tick_id", "-")
    except Exception:
        pass
    print(f"| {svc} | {c} | {name} | {hunger} | {hp} | {sanity} | {status} | {loc} | {tick} | {docker} |")
PYEOF

# ── token 统计（累计值；看增量需对比上一轮）──────────────────────────────────
echo "--- token (累计值) ---"
for f in .test-agents/agent-*/data/logs/token_cost_count.tmp; do
  [ -f "$f" ] || continue
  svc=$(echo "$f" | cut -d/ -f2)
  python3 -c "
import json
try:
    d = json.load(open('$f'))
    for model, m in d.get('summary', {}).get('by_provider_model', {}).items():
        print('%s | %s | calls=%s prompt=%s completion=%s failures=%s' % (
            '$svc', model, m.get('total_calls', '?'), m.get('total_prompt_tokens', '?'),
            m.get('total_completion_tokens', '?'), m.get('total_failures', '?')))
except Exception as e:
    print('$svc | parse_error: %s' % e)
" 2>/dev/null
done | sort

# ── 行为分布（决策动作 + 香农熵，行为坏缩早期信号）─────────────────────────
echo "--- action distribution (last ${INTERVAL_MIN}m) ---"
python3 - "$ROUND_DIR" "${AGENTS[@]}" <<'PYEOF'
import json, math, os, sys
from collections import Counter
base, agents = sys.argv[1], sys.argv[2:]
print("| Agent | 总决策 | 熵 H/Hmax | r | 判读 | 分布 |")
print("| ----- | ------ | --------- | - | ---- | ---- |")
for entry in agents:
    svc, c = entry.split(":", 1)
    acts_file = os.path.join(base, svc + ".actions")
    try:
        acts = [l.strip() for l in open(acts_file, encoding='utf-8') if l.strip()]
    except Exception:
        acts = []
    counts = Counter(acts)
    total = sum(counts.values())
    if total == 0:
        print(f"| {svc} | 0 | - | - | 无决策 | - |")
        continue
    h = -sum((v/total) * math.log2(v/total) for v in counts.values())
    hmax = math.log2(len(counts)) if len(counts) > 1 else 1.0
    ratio = h / hmax if hmax > 0 else 0.0
    band = '健康' if ratio > 0.6 else ('收缩' if ratio >= 0.3 else '坍缩疑似')
    dist = ','.join(f'{a}:{n}' for a, n in counts.most_common())
    print(f"| {svc} | {total} | {h:.2f}/{hmax:.2f} | {ratio:.2f} | {band} | {dist} |")
PYEOF
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
