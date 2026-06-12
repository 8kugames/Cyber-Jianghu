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

# Agent 容器检测
AGENT_COUNT=$(docker ps --filter name=test-agent --filter status=running --format '{{.Names}}' | wc -l | tr -d ' ')
EXPECTED=6  # 根据 docker-compose.yml 中的 agent 数量
if [ "$AGENT_COUNT" -ge "$EXPECTED" ]; then
  echo "SKIP AGENT START: ${AGENT_COUNT} containers already running"
  # 快速健康检查
  HEALTHY=0
  for port in 23341 23342 23343 23344 23345 23349; do
    curl -sf --max-time 5 "http://localhost:$port/api/v1/health" > /dev/null && HEALTHY=$((HEALTHY+1))
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

### 0.4 Pre-flight 决策表

| 检测项 | 已就绪 | 未就绪 |
|--------|--------|--------|
| 镜像 | 跳过 Phase 1.4 | 执行构建 + tag |
| Server | 跳过 Phase 1.3 | 执行启动 |
| Agent 容器 | 跳过 Phase 1.5-1.6 | 执行清理 + 启动 |
| 角色 | 跳过 Phase 2 全部 | 执行角色创建 |

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

```bash
PORTS="23341 23342 23343 23344 23345 23349"
TMPDIR=$(mktemp -d)
echo $PORTS | tr ' ' '\n' | xargs -P 6 -I{} sh -c '
  port={}
  curl -f --max-time 10 -s http://localhost:$port/api/v1/health > /dev/null \
    && echo "OK $port" > '"$TMPDIR"'/$port \
    || echo "FAIL $port" > '"$TMPDIR"'/$port
'
FAILS=$(grep -l "^FAIL" "$TMPDIR"/* 2>/dev/null | sed "s|$TMPDIR/||")
[ -n "$FAILS" ] && { for p in $FAILS; do docker logs --tail 50 test-agent-$((p-23340)) 2>&1 | tail -20; done; exit 1; }
rm -rf "$TMPDIR"
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

目标：每个 agent 创建具有完整背景的新角色。

每端口强依赖：generate→register→verify 必须串行；**端口间完全独立，用 `xargs -P 6` 全部并发**（6 个角色同时创建，总耗时 = 最慢的那个，而非 6 倍）。

```bash
PORTS="23341 23342 23343 23344 23345 23349"
TMPDIR=$(mktemp -d)

printf '%s\n' $PORTS | xargs -P 6 -I{} sh -c '
  port={}; cdir='"$TMPDIR"'/$port; mkdir -p "$cdir"
  # 2.1 generate（失败重试 1 次）
  curl -fsX POST --max-time 60 http://localhost:$port/api/v1/character/generate > "$cdir/gen" || \
    { sleep 2; curl -fsX POST --max-time 60 http://localhost:$port/api/v1/character/generate > "$cdir/gen" || { echo "FAIL generate" > "$cdir/status"; exit 0; }; }
  # 2.2 register
  curl -fsX POST --max-time 60 http://localhost:$port/api/v1/character/register > "$cdir/reg" \
    || { echo "FAIL register" > "$cdir/status"; exit 0; }
  # 2.3 verify
  curl -fs --max-time 30 http://localhost:$port/api/v1/character > "$cdir/char" \
    || { echo "FAIL verify" > "$cdir/status"; exit 0; }
  echo "OK" > "$cdir/status"
'

for port in $PORTS; do
  echo "$port $(cat "$TMPDIR/$port/status" 2>/dev/null || echo TIMEOUT)"
done
```

### 2.4 记录角色信息表

| Agent | 角色 | 年龄 | 性别 | Agent ID |
|-------|------|------|------|----------|

从 `$TMPDIR/$port/char` 解析填充。

部分失败 → 记录失败的 agent，用已成功的继续测试。`rm -rf "$TMPDIR"` 清理。

---

## Phase 3: 运行监控

**监控循环**：

- 总时长：`duration` 参数（默认 24h）
- 检查间隔：`interval` 参数（默认 10min）
- 使用 `ScheduleWakeup` 或 `CronCreate` 调度每轮检查

### 3.1 健康状态检查（每轮）

```bash
PORTS="23341 23342 23343 23344 23345 23349"
TMPDIR=$(mktemp -d)
INTERVAL_MIN=${interval:-10}

printf '%s\n' $PORTS | xargs -P 6 -I{} sh -c '
  port={}; [ "$port" -eq 23349 ] && agent="ollama" || agent="$((port - 23340))"
  cdir='"$TMPDIR"'/$port; mkdir -p "$cdir"
  curl -s --max-time 30 http://localhost:$port/api/v1/character > "$cdir/char" &
  curl -s --max-time 30 http://localhost:$port/api/v1/state     > "$cdir/world" &
  docker ps --filter name=test-agent-$agent --format "{{.Status}}" > "$cdir/docker" &
  wait  # final-join
'
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
printf '%s\n' 23341 23342 23343 23344 23345 23349 | xargs -P 6 -I{} sh -c '
  port={}; [ "$port" -eq 23349 ] && agent="ollama" || agent="$((port - 23340))"
  docker logs --since '"${INTERVAL_MIN}"'m test-agent-$agent 2>&1 | \
    grep -E "ERROR|WARN|死亡|death|panic" | tail -20 > '"$TMPDIR"'/$port.log
'
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
- 容器状态: `docker ps --filter name=test-agent --format '{{.Names}} {{.Status}}'`

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

| 用途 | 方法 | Agent 端点 | Server 端点 |
|------|------|-----------|-------------|
| 健康检查 | GET | `localhost:{port}/api/v1/health` | `localhost:23333/health` |
| 生成角色 | POST | `localhost:{port}/api/v1/character/generate` | - |
| 注册角色 | POST | `localhost:{port}/api/v1/character/register` | - |
| 角色信息 | GET | `localhost:{port}/api/v1/character` | - |
| 世界状态 | GET | `localhost:{port}/api/v1/state` | - |
| 转生 | POST | `localhost:{port}/api/v1/character/rebirth` | - |
| 属性 | GET | `localhost:{port}/api/v1/attributes` | - |

---

## 注意事项

1. **Phase 0 是强制入口**：每次联调必须先执行 Phase 0 智能检测，根据检测结果决定跳过哪些步骤。禁止无条件执行全部步骤。
2. **镜像按 commit hash 缓存**：`agent-agent:{git-short-hash}` tag 用于缓存检测。代码没变就复用，不浪费构建时间。
3. **角色创建必须并行**：6 个 `generate+register+verify` 同时发起（`xargs -P 6`），禁止串行逐个创建。
4. **BuildKit 缓存**：`--no-cache` 不能清除 BuildKit cache mount。如需强制重编译，修改 Dockerfile 中 cache id 或使用 `--build-arg CACHEBUST=$(date +%s)`
5. **Tick 时长**：60s（`game_rules.yaml` 的 `real_seconds_per_tick`），所有 per-tick 计算基于此
6. **Token 持久化**：`$CYBER_JIANGHU_DATA_DIR/logs/token_cost_count.tmp`，每次 tick 由 `persist_and_reset()` 写入
7. **Agent 端口映射**：agent-1=23341, agent-2=23342, ..., agent-5=23345, agent-ollama=23349
8. **容器命名**：test-agent-1 ~ test-agent-5, test-agent-ollama
9. **报告路径**：`logs/测试报告/联调测试.{MMDD}.docker.{N}.md`
10. **配置冻结**：测试开始后不得修改任何 agent 或 server 配置。如需调整，记录变更点并重新开始
