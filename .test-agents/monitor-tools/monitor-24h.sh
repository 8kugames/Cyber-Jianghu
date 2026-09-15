#!/usr/bin/env bash
# dev 24H 联调测试监控循环（Phase 3 数据底座，只观测不干预）
# 每 MON_INTERVAL 秒一轮采集：健康 + 角色状态 + token + 日志错误 + 死亡事件
# 数据写入 .test-agents/logs/dev24h-<timestamp>/
# 端点发现：解析 .test-agents/docker-compose.yml，服务名与 .test-agents/<服务名>/ 数据目录同名

set -u

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

LOG_BASE=".test-agents/logs/dev24h-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$LOG_BASE"
INTERVAL_SECS=${MON_INTERVAL:-600}      # 默认 10 min
DURATION_SECS=${MON_DURATION:-86400}    # 默认 24 h
ROUND=0
START_EPOCH=$(date -u +%s)
END_EPOCH=$((START_EPOCH + DURATION_SECS))

COMPOSE=".test-agents/docker-compose.yml"
# server 地址单一来源 = 实例挂载的 agent.yaml server 段（compose 环境变量已移除）。
# 空值必须 FATAL：否则 curl 相对 URL 静默失败，每轮都会把 server 记为 FAIL，污染监控结论
SERVER_HTTP=$(grep -m1 '^  http_url:' .test-agents/agent-1/config/agent.yaml 2>/dev/null \
  | sed -E 's/^  http_url:[[:space:]]*//' | tr -d '"' | tr -d '[:space:]')
if [ -z "$SERVER_HTTP" ]; then
  echo "FATAL: 无法从 .test-agents/agent-1/config/agent.yaml 解析 server.http_url" >&2
  exit 1
fi
# 输出 "服务名:容器名" 对；服务名即 .test-agents/ 数据目录名，容器名供 docker exec 使用
AGENTS=($(awk '
  BEGIN { in_svc=0; cur=""; cn="" }
  /^services:/ { in_svc=1; next }
  in_svc && /^  [a-z][a-z0-9_-]*:$/ {
    if (cur!="" && cn!="" && cur ~ /^agent-/) print cur":"cn
    cur=$1; sub(/:$/,"",cur); cn=""; next }
  in_svc && /container_name:/ { cn=$2; next }
  END { if (cur!="" && cn!="" && cur ~ /^agent-/) print cur":"cn }' "$COMPOSE"))

if [ ${#AGENTS[@]} -eq 0 ]; then
  echo "FATAL: $COMPOSE 中未发现 agent-* 服务" >&2
  exit 1
fi

log() { echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] $*" | tee -a "$LOG_BASE/monitor.log"; }

collect_round() {
  local round=$1
  local round_log="$LOG_BASE/round-$(printf %04d $round).log"
  local ts
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  echo "=== Round $round @ $ts ===" > "$round_log"

  # --- 1. 容器存活 ---
  {
    echo "[containers]"
    docker ps --filter "name=agent-|name=cyber-jianghu-server" --format "{{.Names}} {{.Status}}"
  } >> "$round_log"

  # --- 2. server health ---
  {
    echo "[server_health]"
    curl -sf --max-time 5 "$SERVER_HTTP/health" || echo "FAIL"
  } >> "$round_log"

  # --- 3. 每 agent 角色状态 + token（容器内 127.0.0.1，token 源 = setup/status 内存权威值）---
  echo "[agents]" >> "$round_log"
  for entry in "${AGENTS[@]}"; do
    local c="${entry##*:}"
    local cdir="$LOG_BASE/agents/$c"
    mkdir -p "$cdir"
    local token
    token=$(docker exec "$c" curl -sf --max-time 5 \
      http://127.0.0.1:23340/api/v1/setup/status 2>/dev/null | \
      jq -r '.auth_token // empty' 2>/dev/null)
    if [ -z "$token" ]; then
      echo "$c: NO_TOKEN" >> "$round_log"
      continue
    fi
    (
      # character
      docker exec "$c" curl -s --max-time 30 \
        -H "Authorization: Bearer $token" \
        http://127.0.0.1:23340/api/v1/character > "$cdir/character.json" 2>/dev/null
      # state
      docker exec "$c" curl -s --max-time 30 \
        -H "Authorization: Bearer $token" \
        http://127.0.0.1:23340/api/v1/state > "$cdir/state.json" 2>/dev/null
    ) &
  done
  wait

  # 解析每 agent 关键字段
  for entry in "${AGENTS[@]}"; do
    local c="${entry##*:}"
    local cdir="$LOG_BASE/agents/$c"
    local name hp hunger is_alive age
    # 空文件时 jq exit 0 + 空输出，[ -s ] 前置守卫保证缺失/空文件回退到 '?'（与旧 python 一致）
    name=$([ -s "$cdir/character.json" ] && jq -r '.name // "?"' "$cdir/character.json" 2>/dev/null || echo '?')
    is_alive=$([ -s "$cdir/character.json" ] && jq -r '.status // "?"' "$cdir/character.json" 2>/dev/null || echo '?')
    hp=$([ -s "$cdir/state.json" ] && jq -r '.self_state.attributes.hp // "?"' "$cdir/state.json" 2>/dev/null || echo '?')
    hunger=$([ -s "$cdir/state.json" ] && jq -r '.self_state.attributes.hunger // "?"' "$cdir/state.json" 2>/dev/null || echo '?')
    location=$([ -s "$cdir/state.json" ] && jq -r '.location.node_id // "?"' "$cdir/state.json" 2>/dev/null || echo '?')
    age=$([ -s "$cdir/character.json" ] && jq -r '.age // "?"' "$cdir/character.json" 2>/dev/null || echo '?')
    echo "  $c name=$name age=$age hp=$hp hunger=$hunger loc=$location alive=$is_alive" >> "$round_log"
  done

  # --- 4. token 统计 ---
  echo "[token_stats]" >> "$round_log"
  for entry in "${AGENTS[@]}"; do
    local svc="${entry%%:*}"
    local c="${entry##*:}"
    local token_file=".test-agents/${svc}/data/logs/token_cost_count.tmp"
    if [ -f "$token_file" ]; then
      # agent token_cost_count.tmp 结构: {"summary": {"by_provider_model": {"<provider>/<model>": {...}}}, "detail": {...}}
      jq -r --arg c "$c" '.summary.by_provider_model | to_entries[] |
        "  \($c).\(.key): prompt=\(.value.total_prompt_tokens // 0) comp=\(.value.total_completion_tokens // 0) calls=\(.value.total_calls // 0) fail=\(.value.total_failures // 0)"' \
        "$token_file" >> "$round_log" 2>/dev/null
    fi
  done

  # --- 5. 日志错误扫描（自上次 round） ---
  echo "[log_errors]" >> "$round_log"
  for entry in "${AGENTS[@]}"; do
    local c="${entry##*:}"
    local err_log="$LOG_BASE/agents/$c/errors.log"
    docker logs --since "${INTERVAL_SECS}s" "$c" 2>&1 \
      | grep -E "ERROR|FATAL|panic|死亡|death" \
      | tail -10 >> "$err_log" 2>/dev/null
    local n
    n=$(wc -l < "$err_log" 2>/dev/null || echo 0)
    echo "  $c total_errors=$n (round+=${INTERVAL_SECS}s)" >> "$round_log"
  done

  # --- 6. 死亡事件（character.is_alive=false）---
  for entry in "${AGENTS[@]}"; do
    local c="${entry##*:}"
    local cdir="$LOG_BASE/agents/$c"
    if grep -q '"is_alive":false' "$cdir/character.json" 2>/dev/null \
       && ! [ -f "$cdir/death_flag" ]; then
      echo "  DEATH $c at $ts" >> "$round_log"
      echo "$ts" > "$cdir/death_flag"
      cat "$cdir/state.json" >> "$round_log" 2>/dev/null
    fi
  done

  # --- 7. 行为分布（决策动作 + 香农熵，行为坏缩早期信号）---
  echo "[actions]" >> "$round_log"
  local entry
  for entry in "${AGENTS[@]}"; do
    local svc="${entry%%:*}"
    local c="${entry##*:}"
    local acts_file="$LOG_BASE/agents/$c/actions.log"
    docker logs --since "${INTERVAL_SECS}s" "$c" 2>&1 \
      | grep -oE "决策: \S+" | sed 's/决策: //' > "$acts_file" 2>/dev/null
    # 行为分布熵：k=1 或 total=0 时 hmax 与旧 python 版一致（1.0 / 0.0）
    if [ -s "$acts_file" ]; then
      jq -rRs --arg agent "$c" '
        split("\n") | map(select(length > 0)) as $acts
        | ($acts | group_by(.) | map({a: .[0], n: length}) | sort_by(-.n)) as $dist
        | ($dist | map(.n) | add // 0) as $total
        | (if $total == 0 then
            {h: 0, hmax: 0, r: 0, band: "无决策", top_a: "-", top_n: 0}
          elif ($dist | length) <= 1 then
            {h: 0, hmax: 1, r: 0, band: "坍缩疑似", top_a: $dist[0].a, top_n: $dist[0].n}
          else
            ($dist | map(.n as $n | ($n / $total) as $p | - ($p * ($p | log2))) | add) as $h
            | (($dist | length) | log2) as $hmax
            | ($h / $hmax) as $r
            | {h: $h, hmax: $hmax, r: $r,
               band: (if $r > 0.6 then "健康" elif $r >= 0.3 then "收缩" else "坍缩疑似" end),
               top_a: $dist[0].a, top_n: $dist[0].n}
          end) as $s
        | "  \($agent) total=\($total) H=\($s.h * 100 | round / 100)/\($s.hmax * 100 | round / 100) r=\($s.r * 100 | round / 100) [\($s.band)] top=\($s.top_a)(\($s.top_n)) | \($dist | map("\(.a):\(.n)") | join(","))"
      ' "$acts_file" >> "$round_log" 2>/dev/null || echo "  $c total=0 H=0/0 r=0 [无决策] top=-(0) | " >> "$round_log"
    else
      echo "  $c total=0 H=0/0 r=0 [无决策] top=-(0) | " >> "$round_log"
    fi
    # 累计 CSV（Phase 4 时间序列用）
    if [ -s "$acts_file" ]; then
      ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
      sort "$acts_file" | uniq -c | awk -v svc="$svc" -v ts="$ts" '{print ts "," svc "," $2 "," $1}' \
        >> "$LOG_BASE/actions.csv"
    fi
  done

  log "Round $round done"
}

log "=== dev 24H 监控循环启动 ==="
log "Log base: $LOG_BASE"
log "Interval: ${INTERVAL_SECS}s | Duration: ${DURATION_SECS}s"
log "Agents(compose): ${AGENTS[*]}"

while [ "$(date -u +%s)" -lt "$END_EPOCH" ]; do
  ROUND=$((ROUND+1))
  collect_round "$ROUND" || log "Round $ROUND FAILED"
  if [ "$(date -u +%s)" -ge "$END_EPOCH" ]; then break; fi
  sleep "$INTERVAL_SECS"
done

log "=== 监控循环结束（${ROUND} 轮）==="
log "STOP_FILE: $LOG_BASE"
