---
name: integration-test
description: "联调测试一键执行：部署验证→角色创建→运行监控→报告生成。完整覆盖 Docker 多 Agent 集成测试全流程，自动采集 token 统计、状态快照、死亡事件等数据并生成结构化报告。Use when: 用户说'联调测试'、'跑测试'、'开始测试'、'生成测试报告'、或输入 /联调测试。"
---

# 联调测试 SKILL

一键执行 Cyber-Jianghu 多 Agent 联调测试。4 阶段流水线：部署→角色创建→监控→报告。

## 参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `duration` | 24h | 测试总时长 |
| `interval` | 10min | 监控检查间隔 |
| `agents` | 全部 | 指定 agent（如 `agent-1,agent-2`） |
| `skip-build` | false | 跳过 rebuild（复用现有镜像） |
| `skip-report` | false | 跳过报告生成 |

## 并行执行（最短提示）

1. **多 bash tool call 同步发起 最优先** — LLM 在同一响应内发 6 个 curl，工具调度器天然并行
2. **单 bash 内 fan-out** 用 `xargs -P N`（macOS / GNU 都可移植），不要 `wait -n`（zsh 5.x / macOS bash 3.2.57 报错）
3. **fan-in 用 final-join 屏障**：`&` 后接裸 `wait` 等所有后台任务完成；不要用 `VAR=$(cmd) &`（后台子 shell 拿不到结果）
4. **强依赖保留串行**：generate→register→verify、build→run、retry backoff（`sleep N` 后重试）

---

## Phase 0: Pre-flight 智能检测

**目标**：跳过已完成步骤，最小化等待时间。每步检测到"已就绪"则直接跳过，不浪费秒。

### 0.0 Agent Endpoints 自动发现（必须最先加载）

从 `.test-agents/docker-compose.yml` 解析所有匹配 `AGENT_PATTERN` 的 service，避免在 skill 内硬编码容器名列表。后续 §0.2/§0.5/§1.7/§2/§3 全部依赖此数组。

可调 env: `COMPOSE_FILE`（默认 `.test-agents/docker-compose.yml`）、`AGENT_PATTERN`（默认 `^agent-`）、`AGENT_INTERNAL_PORT`（默认 `23340`）。

```bash
COMPOSE_FILE="${COMPOSE_FILE:-.test-agents/docker-compose.yml}"
AGENT_PATTERN="${AGENT_PATTERN:-^agent-}"
AGENT_INTERNAL_PORT="${AGENT_INTERNAL_PORT:-23340}"

# 从 compose 提取 (host_port, container_name) 数组，仅匹配 AGENT_PATTERN 的 service
parse_agent_endpoints() {
  awk -v pat="$AGENT_PATTERN" -v internal="$AGENT_INTERNAL_PORT" '
    BEGIN { in_svc=0; cur=""; cn=""; hp="" }
    /^services:/ { in_svc=1; next }
    in_svc && /^  [a-z][a-z0-9_-]*:$/ {
      if (cur!="" && cn!="" && hp!="" && cur ~ pat) print hp, cn
      cur=$1; sub(/:$/, "", cur); cn=""; hp=""; next
    }
    in_svc && /container_name:/ { cn=$2; next }
    in_svc && hp=="" && /\"/ {
      # 匹配 "HOST_PORT:INTERNAL" 形式（短语法）
      n=split($0, q, "\"")
      for(i=1; i<n; i+=2) {
        if (q[i+1] ~ ":" internal "$" && q[i+1] !~ "^:") {
          split(q[i+1], pp, ":")
          if (pp[1]+0 > 0) { hp=pp[1]; break }
        }
      }
    }
    END {
      if (cur!="" && cn!="" && hp!="" && cur ~ pat) print hp, cn
    }
  ' "$COMPOSE_FILE"
}

mapfile -t AGENT_ENDPOINTS < <(parse_agent_endpoints)
[ ${#AGENT_ENDPOINTS[@]} -eq 0 ] && {
  echo "FAIL: no agent services matched pattern '$AGENT_PATTERN' in $COMPOSE_FILE"
  exit 1
}
echo "DISCOVERED: ${#AGENT_ENDPOINTS[@]} agent endpoints from $COMPOSE_FILE"
```

### 0.1 镜像缓存检测

按 git commit hash 检查已有镜像，避免重复构建（节省 3-6 分钟）。

```bash
COMMIT=$(git rev-parse --short HEAD)
EXISTING=$(docker images --format '{{.Repository}}:{{.Tag}}' | grep "agent-agent:${COMMIT}" || true)
if [ -n "$EXISTING" ]; then
  echo "SKIP BUILD: image agent-agent:${COMMIT} already exists"
  # tag 为 latest 供 docker-compose 使用
  docker tag "agent-agent:${COMMIT}" agent-agent:latest 2>/dev/null
else
  echo "NEED BUILD: no image for commit ${COMMIT}"
fi
```

**仅在 NEED BUILD 时**执行 Phase 1.4 的构建流程，构建完成后：
```bash
docker tag agent-agent:latest "agent-agent:${COMMIT}"
```

### 0.2 服务状态检测

检查 server 和 agent 容器是否已 running + healthy。

```bash
# Server 检测
SERVER_HEALTHY=$(curl -sf --max-time 5 http://localhost:23333/health && echo "OK" || echo "FAIL")
if [ "$SERVER_HEALTHY" = "OK" ]; then
  echo "SKIP SERVER START: already healthy"
else
  echo "NEED SERVER START"
fi

# Agent 容器检测（容器内 127.0.0.1 bind，必须 docker exec）
AGENT_COUNT=$(docker ps --filter name=agent- --filter status=running --format '{{.Names}}' | wc -l | tr -d ' ')
EXPECTED=${#AGENT_ENDPOINTS[@]}  # 由 §0.0 定义
if [ "$AGENT_COUNT" -ge "$EXPECTED" ]; then
  echo "SKIP AGENT START: ${AGENT_COUNT} containers already running"
  # 快速健康检查（容器内）
  HEALTHY=0
  for entry in "${AGENT_ENDPOINTS[@]}"; do
    c=$(echo "$entry" | awk '{print $2}')
    code=$(docker exec "$c" curl -s -o /dev/null -w '%{http_code}' \
      --max-time 5 http://127.0.0.1:23340/api/v1/health 2>/dev/null || echo "000")
    [ "$code" = "200" ] && HEALTHY=$((HEALTHY+1))
  done
  echo "Healthy: ${HEALTHY}/${AGENT_COUNT}"
else
  echo "NEED AGENT START: only ${AGENT_COUNT}/${EXPECTED} running"
fi
```

### 0.3 角色状态检测

检查是否已有活跃角色，避免重复创建（节省 2-3 分钟）。

```bash
ACTIVE_AGENTS=$(docker exec cyber-jianghu-postgres psql -U postgres -d cyber_jianghu -t -c \
  "SELECT count(*) FROM agents WHERE status='active';" 2>/dev/null | tr -d ' ')
if [ "$ACTIVE_AGENTS" -ge "$EXPECTED" ] 2>/dev/null; then
  echo "SKIP CHARACTER CREATION: ${ACTIVE_AGENTS} active agents already exist"
else
  echo "NEED CHARACTER CREATION: only ${ACTIVE_AGENTS:-0} active agents"
fi
```

### 0.5 API 协议探测 + auth_token 读取

agent HTTP 容器内 bind `127.0.0.1`，所有 curl 走 `docker exec $c curl http://127.0.0.1:23340/...`。探测结果决定 §2/§3 是否带 Bearer token。

```bash
PROBE_TMPDIR=$(mktemp -d)
NEED_AUTH=0; UNAVAILABLE=0; LEGACY=0
for entry in "${AGENT_ENDPOINTS[@]}"; do
  p=$(echo "$entry" | awk '{print $1}')
  c=$(echo "$entry" | awk '{print $2}')
  (
    code=$(docker exec $c curl -s -o /dev/null -w '%{http_code}' \
      --max-time 5 http://127.0.0.1:23340/api/v1/character 2>/dev/null || echo "000")
    echo "$code" > "$PROBE_TMPDIR/$p.code"
    token=$(docker exec $c grep '^auth_token:' \
      /app/data/servers/cyber-jianghu-server-23333/device.yaml 2>/dev/null \
      | awk '{print $2}')
    echo "$token" > "$PROBE_TMPDIR/$p.token"
  ) &
done
wait
for entry in "${AGENT_ENDPOINTS[@]}"; do
  p=$(echo "$entry" | awk '{print $1}')
  c=$(echo "$entry" | awk '{print $2}')
  code=$(cat "$PROBE_TMPDIR/$p.code" 2>/dev/null)
  case "$code" in
    401) NEED_AUTH=$((NEED_AUTH+1));;           # P0-11(b) 认证层生效
    503) echo "WARN: $c device 未初始化"; UNAVAILABLE=$((UNAVAILABLE+1));;
    200) LEGACY=$((LEGACY+1));;                 # 老版本无认证
    *)   echo "WARN: $c unreachable (code=$code)"; UNAVAILABLE=$((UNAVAILABLE+1));;
  esac
done
rm -rf "$PROBE_TMPDIR"
if [ "$NEED_AUTH" -gt 0 ]; then
  echo "PROTOCOL: Bearer auth required ($NEED_AUTH/${#AGENT_ENDPOINTS[@]} agents)"
elif [ "$LEGACY" -gt 0 ]; then
  echo "PROTOCOL: legacy no-auth ($LEGACY/${#AGENT_ENDPOINTS[@]} agents)"
fi
[ "$UNAVAILABLE" -gt 0 ] && echo "FAIL: $UNAVAILABLE agent(s) unreachable"
```

### 0.6 镜像陈旧检测

对比 git HEAD commit 时间 vs 镜像创建时间（>24h 视为陈旧），避免跑老镜像导致协议不一致。

```bash
COMMIT_TS=$(git log -1 --format=%ct HEAD 2>/dev/null || echo 0)

check_image_freshness() {
  local img="$1"
  local img_epoch=$(docker inspect "$img" --format='{{.Created}}' 2>/dev/null | \
    python3 -c "
import sys, datetime
t = sys.stdin.read().strip()
if t:
    dt = datetime.datetime.fromisoformat(t.replace('Z', '+00:00'))
    print(int(dt.timestamp()))
else:
    print(0)
" 2>/dev/null || echo 0)
  if [ -z "$img_epoch" ] || [ "$img_epoch" = "0" ]; then
    echo "STALE: $img missing"
  elif [ $((COMMIT_TS - img_epoch)) -gt 86400 ]; then
    echo "STALE: $img is $(( (COMMIT_TS - img_epoch) / 86400 )) days old"
  else
    echo "FRESH: $img within 24h"
  fi
}

check_image_freshness cyber-jianghu-server
check_image_freshness agent-agent:latest
```

### 0.4 Pre-flight 决策表

| 检测项 | 已就绪 | 未就绪 |
|--------|--------|--------|
| 镜像 (Phase 0.1) | 跳过 Phase 1.4 | 执行构建 + tag |
| Server 镜像陈旧 (Phase 0.6) | - | 强制 Phase 1.3 重建 |
| Agent 镜像陈旧 (Phase 0.6) | - | 强制 Phase 1.4 重建 |
| Server (Phase 0.2) | 跳过 Phase 1.3 | 执行启动 |
| Agent 容器 (Phase 0.2) | 跳过 Phase 1.5-1.6 | 执行清理 + 启动 |
| API 协议 (Phase 0.5) | 走 Bearer token 路径 | 走裸调路径（兼容老版本） |
| 角色 (Phase 0.3) | 跳过 Phase 2 全部 | 执行角色创建 |

**全部就绪 → 直接跳到 Phase 3（监控）或 Phase 4（报告）。**

---

## Phase 1: 部署验证

目标：所有组件 running + healthy，配置冻结。

> **注意**：Phase 0 检测通过的项目直接跳过对应子步骤。

### 1.1 代码验证

```bash
cargo clippy --workspace --all-targets -- -D warnings > /tmp/clippy.log 2>&1 &
cargo nextest run --workspace > /tmp/nextest.log 2>&1 &
wait  # final-join barrier
grep -qE "^(warning|error):" /tmp/clippy.log && { cat /tmp/clippy.log; exit 1; }
grep -q "FAILED" /tmp/nextest.log && { cat /tmp/nextest.log; exit 1; }
```

任一失败 → 中断，报告失败原因。

### 1.2 Docker 网络检查

```bash
docker network inspect cyber-jianghu-network
```

不存在则创建：

```bash
docker network create cyber-jianghu-network
```

### 1.3 Server 启动

```bash
./install.sh all start
curl -f http://localhost:23333/health
docker inspect cyber-jianghu-server --format '{{.Config.Image}} {{.Created}}'
git rev-parse --short HEAD
```

### 1.4 Agent 镜像构建

**仅在 Phase 0.1 检测为 NEED BUILD 时执行此步骤。**

**BuildKit 缓存污染防护**：BuildKit 的 `--mount=type=cache` 不受 `--no-cache` 和 `docker builder prune` 影响。必须通过改 cache id 强制重编译。

```bash
docker compose -f .test-agents/docker-compose.yml down
docker rmi agent-agent:latest 2>/dev/null
DOCKER_BUILDKIT=1 docker compose -f .test-agents/docker-compose.yml build --no-cache
docker inspect agent-agent:latest --format '{{.Created}}'  # 验证时间戳是刚才
# 构建完成后打 commit hash tag
COMMIT=$(git rev-parse --short HEAD)
docker tag agent-agent:latest "agent-agent:${COMMIT}"
```

**缓存污染检测**：

- 对比二进制 hash：`docker run --rm agent-agent:latest md5sum /app/agent` 与上次不同
- 如果怀疑缓存未失效，修改 Dockerfile 中 `--mount=type=cache,id=...` 的 id 值后重建

### 1.5 清理旧数据

**仅在 Phase 0.2 检测为 NEED AGENT START 时执行此步骤。**

```bash
rm -rf .test-agents/agent-*/data/{logs,*.db*,*.json}  # 保留 config
```

### 1.6 启动全部 Agent

```bash
docker compose -f .test-agents/docker-compose.yml up -d
```

### 1.7 健康检查

容器内 `docker exec + 127.0.0.1`，`/api/v1/health` 公开，无须 Bearer。

```bash
TMPDIR=$(mktemp -d)
for entry in "${AGENT_ENDPOINTS[@]}"; do
  p=$(echo "$entry" | awk '{print $1}')
  c=$(echo "$entry" | awk '{print $2}')
  (
    code=$(docker exec $c curl -s -o /dev/null -w '%{http_code}' \
      --max-time 10 http://127.0.0.1:23340/api/v1/health 2>/dev/null || echo "000")
    if [ "$code" = "200" ]; then
      echo "OK" > "$TMPDIR/$p"
    else
      echo "FAIL $code" > "$TMPDIR/$p"
    fi
  ) &
done
wait
FAILS=""
for entry in "${AGENT_ENDPOINTS[@]}"; do
  p=$(echo "$entry" | awk '{print $1}')
  c=$(echo "$entry" | awk '{print $2}')
  if [ "$(cat "$TMPDIR/$p" 2>/dev/null)" != "OK" ]; then
    FAILS="$FAILS $c"
  fi
done
rm -rf "$TMPDIR"
if [ -n "$FAILS" ]; then
  for c in $FAILS; do
    docker logs --tail 50 "$c" 2>&1 | tail -20
  done
  exit 1
fi
```

### 1.8 记录基线信息

记录到报告 §9.1：

- Git commit hash
- Server 镜像 tag + 创建时间
- Agent 镜像 tag + 创建时间
- Agent 配置矩阵（从各 `agent.yaml` 读取 model/provider/fallback/temperature）

---

## Phase 2: 角色创建

**仅在 Phase 0.3 检测为 NEED CHARACTER CREATION 时执行此步骤。**

每端口强依赖 generate→register→verify 串行；端口间用 `&` + `wait` 全并发，总耗时 = 最慢的那个。

```bash
TMPDIR=$(mktemp -d)
TOKEN_MAP="$TMPDIR/tokens"
> "$TOKEN_MAP"
for entry in "${AGENT_ENDPOINTS[@]}"; do
  c=$(echo "$entry" | awk '{print $2}')
  echo "$c ${AGENT_TOKENS[$c]}" >> "$TOKEN_MAP"
done

for entry in "${AGENT_ENDPOINTS[@]}"; do
  p=$(echo "$entry" | awk '{print $1}')
  c=$(echo "$entry" | awk '{print $2}')
  (
    cdir="$TMPDIR/$p"
    mkdir -p "$cdir"
    token=$(awk -v cn="$c" '$1==cn{print $2}' "$TOKEN_MAP")
    auth_args=()
    [ -n "$token" ] && auth_args=(-H "Authorization: Bearer $token")

    # 2.1 generate（失败重试 1 次）
    docker exec "$c" curl -fsX POST --max-time 60 "${auth_args[@]}" \
      http://127.0.0.1:23340/api/v1/character/generate > "$cdir/gen" 2>/dev/null || \
      { sleep 2; docker exec "$c" curl -fsX POST --max-time 60 "${auth_args[@]}" \
        http://127.0.0.1:23340/api/v1/character/generate > "$cdir/gen" 2>/dev/null \
        || { echo "FAIL generate" > "$cdir/status"; exit 0; }; }
    # 2.2 register
    docker exec "$c" curl -fsX POST --max-time 60 "${auth_args[@]}" \
      http://127.0.0.1:23340/api/v1/character/register > "$cdir/reg" 2>/dev/null \
      || { echo "FAIL register" > "$cdir/status"; exit 0; }
    # 2.3 verify
    docker exec "$c" curl -fs --max-time 30 "${auth_args[@]}" \
      http://127.0.0.1:23340/api/v1/character > "$cdir/char" 2>/dev/null \
      || { echo "FAIL verify" > "$cdir/status"; exit 0; }
    echo "OK" > "$cdir/status"
  ) &
done
wait
rm -f "$TOKEN_MAP"

for entry in "${AGENT_ENDPOINTS[@]}"; do
  p=$(echo "$entry" | awk '{print $1}')
  status=$(cat "$TMPDIR/$p/status" 2>/dev/null || echo "TIMEOUT")
  echo "$p $status"
done
```

### 2.4 记录角色信息表

| Agent | 角色 | 年龄 | 性别 | Agent ID |
|-------|------|------|------|----------|

从 `$TMPDIR/$p/char` 解析填充。

部分失败 → 记录失败的 agent，用已成功的继续测试。`rm -rf "$TMPDIR"` 清理。

---

## Phase 3: 运行监控

**监控循环**：

- 总时长：`duration` 参数（默认 24h）
- 检查间隔：`interval` 参数（默认 10min）
- 使用 `ScheduleWakeup` 或 `CronCreate` 调度每轮检查

### 3.1 健康状态检查（每轮）

容器内 `docker exec + 127.0.0.1` + Bearer token。

```bash
TMPDIR=$(mktemp -d)
INTERVAL_MIN=${interval:-10}
TOKEN_MAP="$TMPDIR/tokens"
> "$TOKEN_MAP"
for entry in "${AGENT_ENDPOINTS[@]}"; do
  c=$(echo "$entry" | awk '{print $2}')
  echo "$c ${AGENT_TOKENS[$c]}" >> "$TOKEN_MAP"
done

for entry in "${AGENT_ENDPOINTS[@]}"; do
  p=$(echo "$entry" | awk '{print $1}')
  c=$(echo "$entry" | awk '{print $2}')
  (
    cdir="$TMPDIR/$p"
    mkdir -p "$cdir"
    token=$(awk -v cn="$c" '$1==cn{print $2}' "$TOKEN_MAP")
    auth_args=()
    [ -n "$token" ] && auth_args=(-H "Authorization: Bearer $token")

    docker exec "$c" curl -s --max-time 30 "${auth_args[@]}" \
      http://127.0.0.1:23340/api/v1/character > "$cdir/char" &
    docker exec "$c" curl -s --max-time 30 "${auth_args[@]}" \
      http://127.0.0.1:23340/api/v1/state     > "$cdir/world" &
    docker ps --filter name="$c" --format "{{.Status}}" > "$cdir/docker" &
    wait  # final-join
  ) &
done
wait
rm -f "$TOKEN_MAP"
```

记录到监控日志表：

| Agent | Hunger | HP | Sanity | 状态 | 备注 |
|-------|--------|-----|--------|------|------|

### 3.2 Token 数据采集（每轮）

```bash
ls .test-agents/agent-*/data/logs/token_cost_count.tmp 2>/dev/null | \
  xargs -P 6 -I{} sh -c 'cat {} 2>/dev/null'
```

解析 JSON，记录每个 model 的 prompt_tokens / completion_tokens / calls / failures。

### 3.3 日志检查（每轮）

```bash
TMPDIR=$(mktemp -d)
INTERVAL_MIN=${interval:-10}
for entry in "${AGENT_ENDPOINTS[@]}"; do
  p=$(echo "$entry" | awk '{print $1}')
  c=$(echo "$entry" | awk '{print $2}')
  (
    docker logs --since "${INTERVAL_MIN}m" "$c" 2>&1 | \
      grep -E "ERROR|WARN|死亡|death|panic" | tail -20 > "$TMPDIR/$p.log"
  ) &
done
wait
```

### 3.4 死亡处理

检测条件：`GET /api/v1/character` 返回 status 非 active，或 WorldState 中 `is_alive=false`。

处理：记录（agent/tick/hunger/HP/sanity/最近行为）→ 等待 `auto_rebirth` 或手动 `POST /api/v1/character/rebirth` → 重新走 Phase 2 → 记入死亡事件表。

### 3.5 恶性 Bug 判定

满足任一 → **立即中断**：关键流程无法推进（创建角色/action 死循环）/ 数据损坏不可恢复 / >50% agent 同时异常。

中断处理：`docker compose ... stop` → 记录复现步骤 → 报告用户。

### 3.6 监控轮次记录格式

每轮检查记录为报告的一个子节：

```
### T+{hours}h 检查 (~{time} CST)

{简要描述本轮关键事件}

| Agent | Hunger | HP | Sanity | 状态 |
|-------|--------|-----|--------|------|
| ... | ... | ... | ... | ... |
```

---

## Phase 4: 报告生成

目标：按 §9.1-§9.7 模板生成数据增强的测试报告。

### 4.1 自动编号

```bash
ls logs/测试报告/联调测试.{mmdd}.*.md 2>/dev/null
```

编号规则：`联调测试.{MMDD}.docker.{N}.md`，N 从 1 递增。

### 4.2 数据采集

**基础数据**（每个字段独立命令，LLM 直接发起多个并发 bash tool call 拿全）：

- Git commit: `git rev-parse --short HEAD`
- 测试起止时间 / 监控轮次记录 / 各 agent 角色信息
- Server 镜像: `docker inspect cyber-jianghu-server --format '{{.Config.Image}} {{.Created}}'`
- Agent 镜像: `docker inspect agent-agent:latest --format '{{.Created}}'`
- 容器状态: `docker ps --filter name=agent- --format '{{.Names}} {{.Status}}'`

**Token 统计**（per-agent 隔离，跨阶段复用 `$REPORT_TMPDIR/tokens/`）：

```bash
REPORT_TMPDIR=$(mktemp -d); mkdir -p "$REPORT_TMPDIR/tokens"
ls -d .test-agents/agent-*/ 2>/dev/null | xargs -P 6 -I{} sh -c '
  d={}; name=$(basename "$d")
  [ -f "$d/data/logs/token_cost_count.tmp" ] && \
    cp "$d/data/logs/token_cost_count.tmp" "'"$REPORT_TMPDIR"'/tokens/${name}.tokens"
'
```

| Agent | Model | Prompt Tokens | Completion Tokens | Total Tokens | Calls | Failures |

Agent 运行统计（WorldState + 日志）：总决策数（tick × agent）、死亡次数、Rebirth 成功率。

### 4.3 报告结构（§9.1-§9.7）

- **§9.1 验收参数区**：测试时长、SLO、Agent 配置矩阵、镜像信息、角色信息表
- **§9.2 执行摘要**：总运行时长、决策数、死亡/rebirth 统计、容器稳定性（数据增强：Token 消耗明细表）
- **§9.3 H1 自主决策结论**：ActionSuccessRate、失败模式 Top-K
- **§9.4 H2 涌现结论**：系统级行为描述（从监控日志中提取涌现事件）
- **§9.5 H3 技术可行性结论**：延迟、错误率、成本（数据增强：per-agent calls/tick、tokens/tick、tokens/call 统计）
- **§9.6 模型选型**：各 provider 表现对比（从 token 统计推导：tokens/call、failure rate、fallback 频率）
- **§9.7 问题清单**：P0/P1/P2 分级

### 4.4 Token 数据增强表格格式

在 §9.2 中增加：

```
### Token 消耗明细

| Agent | Model | Prompt | Completion | Total | Calls | Failures | Tokens/Call |
|-------|-------|--------|------------|-------|-------|----------|-------------|
```

在 §9.5 中增加：

```
### 运行效率统计

| Agent | Total Calls | Calls/Tick | Tokens/Tick | Tokens/Call |
|-------|-------------|------------|-------------|-------------|
```

Tick 数计算：`运行秒数 / 60`（real_seconds_per_tick = 60）。

---

## 关键 API 端点速查

**Agent API 必须在容器内调用**（127.0.0.1 bind + Bearer token）：

```bash
# 模板
docker exec $c curl -s -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:23340<path>
```

| 用途 | 方法 | Agent 路径（容器内） | Server 路径（宿主机） |
|------|------|---------------------|----------------------|
| 健康检查 | GET | `/api/v1/health`（公开，无须 Bearer） | `localhost:23333/health` |
| 生成角色 | POST | `/api/v1/character/generate` | - |
| 注册角色 | POST | `/api/v1/character/register` | - |
| 角色信息 | GET | `/api/v1/character` | - |
| 世界状态 | GET | `/api/v1/state` | - |
| 转生 | POST | `/api/v1/character/rebirth` | - |
| 属性 | GET | `/api/v1/attributes` | - |

---

## 注意事项

1. **Phase 0 是强制入口**：检测通过的项目直接跳过对应 Phase，禁止无条件执行全流程。
2. **镜像按 commit hash 缓存**：`agent-agent:{short-hash}` tag 用于 Phase 0.1 检测，代码未变不重新构建。
3. **角色创建必须并行**：端口间用 `&` + `wait` 全并发；每端口内 generate→register→verify 串行（含 1 次重试）。
4. **BuildKit 缓存**：`--no-cache` 不能清除 BuildKit cache mount；需改 Dockerfile 的 `--mount=type=cache,id=...` 或加 `--build-arg CACHEBUST=$(date +%s)`。
5. **Tick 时长**：60s（`game_rules.yaml` 的 `real_seconds_per_tick`）。
6. **agent HTTP 协议（P0-11 a/b）**：容器内 bind `127.0.0.1` + 除公开路径外强制 Bearer token。宿主机 `localhost:$port` 会被 docker-proxy 拒绝，必须 `docker exec $c curl http://127.0.0.1:23340/...`。token 来自容器内 `/app/data/servers/cyber-jianghu-server-23333/device.yaml`。**禁止改回 `0.0.0.0`**。
7. **AGENT_ENDPOINTS**：由 Phase 0.0 从 compose 自动解析；增删 agent 改 `.test-agents/docker-compose.yml` 即可。
8. **配置冻结**：测试开始后不得修改任何 agent 或 server 配置；如需调整，记录变更点并重新开始。
