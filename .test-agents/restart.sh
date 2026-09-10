#!/bin/bash
# restart.sh - 联调测试一站式工具：镜像构建(离线) + 容器重启 + 角色归隐/注册 + 健康验证
#
# 用法:
#   ./restart.sh                 # 重启所有 agent（保留现有角色）
#   ./restart.sh --register      # 重启 + 归隐旧角色 + 注册全新角色 + 验证
#   ./restart.sh --build         # 重启前离线重建镜像（本地基镜像通道，无需外网）
#   ./restart.sh --no-register   # 重启 + 跳过注册
#   ./restart.sh agent-1         # 只操作指定 agent
#   ./restart.sh --build --register agent-1   # 组合使用
#
# 依赖: docker, curl, python3
# 设计约定:
#   - 确定性操作全部在本脚本内完成；联调测试 SKILL 只负责调用本脚本、分析输出、异常处置
#   - token 源 = agent 的 GET /api/v1/setup/status（内存权威值）。
#     device.yaml 中的 auth_token 会因 server 端轮换而滞后，禁止用作调用凭证
#   - 归隐 = server 端 POST /api/v1/agent/retire（幂等），先于注册执行，
#     满足 server 0.1.296+ 的「单设备单活跃角色」约束
#   - server 地址从 docker-compose.yml 解析；换 server 只改 compose，本脚本零修改

set -uo pipefail
cd "$(dirname "$0")"

# ── 配置 ──────────────────────────────────────────────────────────────────────
AGENTS=(
  "agent-1:23341"
  "agent-2:23342"
  "agent-3:23343"
  "agent-4:23344"
)

COMPOSE="docker-compose.yml"
READY_TIMEOUT=30
GEN_RETRY_MAX=4
GEN_MAX_TIME=180
SERVER_RETIRE_RETRY=5
SERVER_RETIRE_SLEEP=5
TMPDIR="./.tmp/restart_$$"

trap 'rm -rf "$TMPDIR" 2>/dev/null; exit 130' INT TERM

# ── server 配置解析（单一来源 = compose）─────────────────────────────────────
SERVER_HTTP=$(grep -m1 'CYBER_JIANGHU_SERVER_HTTP_URL:' "$COMPOSE" | sed -E 's/.*CYBER_JIANGHU_SERVER_HTTP_URL:[[:space:]]*//')
if [ -z "$SERVER_HTTP" ]; then
  echo "FATAL: 无法从 $COMPOSE 解析 CYBER_JIANGHU_SERVER_HTTP_URL" >&2
  exit 1
fi
# server_key 与 agent 端 config.rs 的 server_key() 规则一致: host 点转横线 + -port
SERVER_HOST=$(echo "$SERVER_HTTP" | sed -E 's|https?://||; s|:[0-9]+$||')
SERVER_PORT=$(echo "$SERVER_HTTP" | sed -E 's|.*:([0-9]+)$|\1|')
SERVER_KEY="${SERVER_HOST//./-}-${SERVER_PORT}"

# ── 参数解析 ──────────────────────────────────────────────────────────────────
FORCE_REGISTER=""
TARGET_AGENT=""
DO_BUILD=false

for arg in "$@"; do
  case "$arg" in
    --register)     FORCE_REGISTER="yes" ;;
    --no-register)  FORCE_REGISTER="no" ;;
    --build)        DO_BUILD=true ;;
    --help|-h)
      echo "用法: $0 [--register|--no-register] [--build] [agent-name]"
      echo ""
      echo "选项:"
      echo "  --register      归隐旧角色（server 端）+ 注册全新角色 + 验证"
      echo "  --no-register   跳过注册"
      echo "  --build         重启前离线重建镜像（本地基镜像通道）"
      echo "  agent-name      只操作指定 agent（如 agent-1）"
      exit 0
      ;;
    -*)
      echo "未知选项: $arg" >&2
      exit 1
      ;;
    *)
      TARGET_AGENT="$arg"
      ;;
  esac
done

# ── 颜色输出 ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
GRAY='\033[0;90m'
BOLD='\033[1m'
NC='\033[0m'

log_ok()   { echo -e "${GREEN}[OK]${NC} $*"; }
log_fail() { echo -e "${RED}[FAIL]${NC} $*"; }
log_info() { echo -e "${CYAN}[INFO]${NC} $*"; }

emit() {
  local aname=$1 phase=$2 msg=$3
  local ts
  ts=$(date '+%H:%M:%S')
  echo -e "${GRAY}${ts}${NC} ${BOLD}[${aname}]${NC} ${phase} ${msg}"
}

# ── token（内存权威值，来自 setup/status 公开端点）───────────────────────────
current_token() {
  local port=$1
  # 静态 token（不轮换）优先，源自 .env；回退 setup/status 内存值
  if [ -s ".env" ]; then
    grep '^CYBER_JIANGHU_AGENT_TOKEN=' ".env" | head -1 | cut -d= -f2-
    return 0
  fi
  curl -sf "http://localhost:${port}/api/v1/setup/status" --max-time 5 2>/dev/null | \
    python3 -c "import json,sys; print(json.load(sys.stdin).get('auth_token',''))" 2>/dev/null
}

device_id_of() {
  local aname=$1
  grep '^device_id:' "${aname}/data/servers/${SERVER_KEY}/device.yaml" 2>/dev/null | awk '{print $2}'
}

# ── 离线构建（本地基镜像通道，自举基镜像）────────────────────────────────────
ensure_base_images() {
  local need_rust=0 need_slim=0
  docker image inspect local-rust-trixie:builder > /dev/null 2>&1 || need_rust=1
  docker image inspect local-debian-slim:runtime > /dev/null 2>&1 || need_slim=1
  if [ $need_rust -eq 0 ] && [ $need_slim -eq 0 ]; then
    log_ok "本地基镜像已就绪 (local-rust-trixie:builder / local-debian-slim:runtime)"
    return 0
  fi

  log_info "本地基镜像缺失，自举制备（docker run 网络路径，apt 可用）..."
  if [ $need_rust -eq 1 ]; then
    docker run --name tmp-rust-builder rust:trixie bash -c \
      "apt-get update -qq && apt-get install -y -qq pkg-config libssl-dev > /dev/null && rm -rf /var/lib/apt/lists/*" \
      && docker commit tmp-rust-builder local-rust-trixie:builder && docker rm tmp-rust-builder > /dev/null \
      || { log_fail "local-rust-trixie:builder 制备失败"; return 1; }
  fi
  if [ $need_slim -eq 1 ]; then
    docker run --name tmp-deb-slim debian:trixie-slim bash -c \
      "apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates curl libssl3 > /dev/null && rm -rf /var/lib/apt/lists/*" \
      && docker commit tmp-deb-slim local-debian-slim:runtime && docker rm tmp-deb-slim > /dev/null \
      || { log_fail "local-debian-slim:runtime 制备失败"; return 1; }
  fi
  log_ok "本地基镜像制备完成"
}

build_image_offline() {
  ensure_base_images || return 1

  local dockerfile="/tmp/Dockerfile.offline.$$"
  # 由本脚本生成离线版 Dockerfile；结构需与 crates/agent/Dockerfile 保持同步
  # 差异: FROM 指向本地基镜像；apt 步骤已烘焙进基镜像故省略
  cat > "$dockerfile" <<'DOCKERFILE'
# ============================================================================
# TEMP offline Dockerfile — 由 restart.sh 生成，勿手工编辑
# ============================================================================
FROM local-rust-trixie:builder AS builder

ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
ENV RUSTUP_DIST_SERVER=https://mirrors.aliyun.com/rustup
ENV RUSTUP_UPDATE_ROOT=https://mirrors.aliyun.com/rustup
ENV CARGO_REGISTRIES_ALIYUN_INDEX=https://mirrors.aliyun.com/crates.io-index/ \
    CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

WORKDIR /app

RUN mkdir -p ./.cargo \
    && echo '[source.crates-io]\n\
replace-with = "aliyun"\n\
\n\
[source.aliyun]\n\
registry = "sparse+https://mirrors.aliyun.com/crates.io-index/"' > ./.cargo/config.toml

COPY Cargo.toml Cargo.lock* ./
COPY crates/protocol/Cargo.toml ./crates/protocol/
COPY crates/agent/Cargo.toml ./crates/agent/
COPY crates/embedding/Cargo.toml ./crates/embedding/
COPY crates/server/Cargo.toml ./crates/server/

RUN mkdir -p crates/server/src && echo "fn main() {}" > crates/server/src/main.rs && \
    mkdir -p crates/embedding/src && echo "" > crates/embedding/src/lib.rs

COPY crates/protocol/src ./crates/protocol/src
COPY crates/embedding/src ./crates/embedding/src
COPY crates/agent/src ./crates/agent/src

RUN cargo build --release -p cyber-jianghu-agent && \
    cp /app/target/release/cyber-jianghu-agent /app/agent-bin

FROM local-debian-slim:runtime

RUN groupadd -g 1000 cyberjianghu && \
    useradd -u 1000 -g cyberjianghu -m -s /bin/bash cyberjianghu

WORKDIR /app

COPY --from=builder /app/agent-bin /app/agent
COPY crates/agent/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh
COPY crates/agent/static /app/static
RUN mkdir -p /app/data /app/config && \
    chown -R cyberjianghu:cyberjianghu /app

EXPOSE 23340
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:23340/api/v1/health || exit 1
ENV RUST_LOG=info \
    CYBER_JIANGHU_CONFIG_DIR=/app/config \
    CYBER_JIANGHU_DATA_DIR=/app/data
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["./agent", "run", "--port", "23340"]
DOCKERFILE

  log_info "离线构建 agent 镜像（无 cache mount，全量编译约 15-25 分钟）..."
  # 项目根 = 本脚本目录的上级（restart.sh 位于 .test-agents/）
  ( cd .. && DOCKER_BUILDKIT=0 docker build -f "$dockerfile" -t agent-agent:latest . )
  local rc=$?
  rm -f "$dockerfile"
  return $rc
}

# ── server 端归隐（幂等，先于注册，满足单设备单活跃角色约束）─────────────────
server_retire() {
  local aname=$1 token=$2
  local did
  did=$(device_id_of "$aname")
  if [ -z "$did" ]; then
    log_fail "[$aname] device_id 缺失（${aname}/data/servers/${SERVER_KEY}/device.yaml）"
    return 1
  fi

  # 设备 token 在 server 端轮换：先经 /device/verify 现取权威值，再归隐
  local attempt fresh result
  for attempt in $(seq 1 $SERVER_RETIRE_RETRY); do
    fresh=$(curl -sf --max-time 15 -X POST "$SERVER_HTTP/api/v1/device/verify" \
      -H 'Content-Type: application/json' \
      -d "{\"device_id\": \"$did\"}" 2>/dev/null | \
      python3 -c "import json,sys; print(json.load(sys.stdin).get('auth_token',''))" 2>/dev/null || true)
    [ -z "$fresh" ] && { sleep "$SERVER_RETIRE_SLEEP"; continue; }

    result=$(curl -sf --max-time 30 -X POST "$SERVER_HTTP/api/v1/agent/retire" \
      -H 'Content-Type: application/json' \
      -d "{\"device_id\": \"$did\", \"auth_token\": \"$fresh\"}" 2>/dev/null) \
      && [ -n "$result" ] && { echo "$result"; return 0; }
    sleep "$SERVER_RETIRE_SLEEP"
  done
  return 1
}

get_port_by_aname() {
  local aname=$1 entry
  for entry in "${AGENTS[@]}"; do
    IFS=':' read -r svc port <<< "$entry"
    [ "$svc" = "$aname" ] && { echo "$port"; return 0; }
  done
}

# ── 角色生成 + 注册 + 验证 ────────────────────────────────────────────────────
do_generate_register() {
  local port=$1 aname=$2 token=$3
  local gen_file="$TMPDIR/${aname}.gen.json"

  # token 启动后可能被轮换，调用点现读（setup/status 权威值）
  token=$(current_token "$port")

  local gen_ok=false attempt
  for attempt in $(seq 1 $((GEN_RETRY_MAX + 1))); do
    emit "$aname" "${CYAN}LLM${NC}" "正在生成角色（尝试 ${attempt}/$((GEN_RETRY_MAX + 1))，最长 ${GEN_MAX_TIME}s）..."
    if curl -sf -X POST "http://localhost:${port}/api/v1/character/generate" \
      -H "Content-Type: application/json" \
      -H "Authorization: Bearer ${token}" \
      -d '{}' \
      -o "$gen_file" \
      --max-time "$GEN_MAX_TIME" 2>/dev/null && \
       [ -s "$gen_file" ] && \
       python3 -c "import json; d=json.load(open('${gen_file}')); assert d.get('name')" 2>/dev/null; then
      gen_ok=true
      break
    fi
    [ $attempt -le $GEN_RETRY_MAX ] && sleep 5
  done

  if [ "$gen_ok" = false ]; then
    echo "FAIL|$aname|角色生成失败（已重试 ${GEN_RETRY_MAX} 次）" > "$TMPDIR/${aname}.result"
    emit "$aname" "${RED}FAIL${NC}" "角色生成失败（已重试 ${GEN_RETRY_MAX} 次）"
    return 1
  fi

  local gen_name
  gen_name=$(python3 -c "import json; print(json.load(open('${gen_file}')).get('name','?'))" 2>/dev/null || true)
  emit "$aname" "${GREEN}GEN${NC}" "生成角色: ${gen_name}"

  # 注册前现读 token（generate 耗时期间 token 可能再次轮换）
  token=$(current_token "$port")

  emit "$aname" "${CYAN}REG${NC}" "注册到服务器..."
  local reg_result
  reg_result=$(curl -sf -X POST "http://localhost:${port}/api/v1/character/register" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer ${token}" \
    -d @"$gen_file" \
    --max-time 60 2>/dev/null || true)

  if [ -z "$reg_result" ]; then
    echo "FAIL|$aname|角色注册失败（无响应）" > "$TMPDIR/${aname}.result"
    emit "$aname" "${RED}FAIL${NC}" "角色注册失败（无响应）"
    return 1
  fi

  # 注册后验证：character 可查询且 alive
  sleep 2
  token=$(current_token "$port")
  local verify
  verify=$(curl -sf --max-time 15 -H "Authorization: Bearer ${token}" \
    "http://localhost:${port}/api/v1/character" 2>/dev/null | \
    python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('name',''), d.get('agent_id',''), d.get('status',''))" 2>/dev/null || true)

  if echo "$verify" | grep -q "alive"; then
    local vname vaid
    vname=$(echo "$verify" | awk '{print $1}')
    vaid=$(echo "$verify" | awk '{print $2}')
    echo "OK|$aname|${vname} (${vaid})" > "$TMPDIR/${aname}.result"
    emit "$aname" "${GREEN}DONE${NC}" "注册并验证成功: ${vname} (${vaid})"
    return 0
  else
    echo "FAIL|$aname|注册后验证失败: ${verify:-无响应}" > "$TMPDIR/${aname}.result"
    emit "$aname" "${RED}FAIL${NC}" "注册后验证失败"
    return 1
  fi
}

# 单个 agent 完整流程（调用前 API 已确认就绪）
process_agent() {
  local aname=$1 port=$2 token=$3
  local outfile="$TMPDIR/${aname}.result"

  if [ "$FORCE_REGISTER" = "no" ]; then
    echo "SKIP|$aname|跳过注册" > "$outfile"
    emit "$aname" "${GRAY}SKIP${NC}" "跳过注册"
    return
  fi

  # server 端归隐（无条件、幂等）：清掉「单设备单活跃角色」约束，
  # 无论 agent 本地视角是 alive/dead/none 都适用
  emit "$aname" "${YELLOW}RETIRE${NC}" "server 端归隐旧角色（幂等）..."
  if ! server_retire "$aname" "$token"; then
    echo "FAIL|$aname|server 端归隐失败（已重试 ${SERVER_RETIRE_RETRY} 次）" > "$outfile"
    emit "$aname" "${RED}FAIL${NC}" "server 端归隐失败"
    return
  fi

  do_generate_register "$port" "$aname" "$token"
}

# ── 函数：等待 agent API 就绪 ────────────────────────────────────────────────
wait_for_agent() {
  local port=$1
  local elapsed=0
  while [ $elapsed -lt $READY_TIMEOUT ]; do
    if curl -sf "http://localhost:${port}/api/v1/setup/status" -o /dev/null --max-time 2 2>/dev/null; then
      return 0
    fi
    sleep 2
    elapsed=$((elapsed + 2))
  done
  return 1
}

# ── 主流程 ────────────────────────────────────────────────────────────────────

echo "=========================================="
echo " Cyber-Jianghu Agent Restart Tool"
echo "  server: $SERVER_HTTP (key: $SERVER_KEY)"
echo "=========================================="
echo ""

if [ "$DO_BUILD" = true ]; then
  build_image_offline
  if [ $? -ne 0 ]; then
    log_fail "镜像构建失败，中止"
    exit 1
  fi
  log_ok "构建完成，镜像 agent-agent:latest 已更新"
fi

log_info "重启 Docker 容器..."
if [ -n "$TARGET_AGENT" ]; then
  found=false
  for entry in "${AGENTS[@]}"; do
    IFS=':' read -r aname _ <<< "$entry"
    if [ "$aname" = "$TARGET_AGENT" ]; then
      found=true
      break
    fi
  done
  if [ "$found" = false ]; then
    log_fail "未知 agent: $TARGET_AGENT"
    echo "可用: $(for e in "${AGENTS[@]}"; do IFS=':' read -r n _ <<< "$e"; echo -n "$n "; done)"
    exit 1
  fi
  docker compose -f "$COMPOSE" restart "$TARGET_AGENT"
else
  docker compose -f "$COMPOSE" restart
fi
log_ok "容器已重启"
sleep 5  # 等待 device verify / token 刷新完成

echo ""
mkdir -p "$TMPDIR"

# 阶段 1: 并行等待所有 agent API 就绪
log_info "阶段 1/3: 等待所有 agent API 就绪..."
ready_pids=()
for entry in "${AGENTS[@]}"; do
  IFS=':' read -r aname port <<< "$entry"
  if [ -n "$TARGET_AGENT" ] && [ "$aname" != "$TARGET_AGENT" ]; then
    continue
  fi
  (
    if wait_for_agent "$port"; then
      echo "READY|$aname" > "$TMPDIR/${aname}.ready"
      emit "$aname" "${GREEN}READY${NC}" "API 已就绪"
    else
      echo "FAIL|$aname" > "$TMPDIR/${aname}.ready"
      emit "$aname" "${RED}FAIL${NC}" "API 未就绪 (${READY_TIMEOUT}s 超时)"
    fi
  ) &
  ready_pids+=($!)
done
for pid in "${ready_pids[@]}"; do
  wait "$pid" 2>/dev/null || true
done

# 检查哪些 agent 就绪了
alive_agents=()
for entry in "${AGENTS[@]}"; do
  IFS=':' read -r aname port <<< "$entry"
  if [ -n "$TARGET_AGENT" ] && [ "$aname" != "$TARGET_AGENT" ]; then
    continue
  fi
  if [ -f "$TMPDIR/${aname}.ready" ]; then
    rstatus=$(cut -d'|' -f1 < "$TMPDIR/${aname}.ready")
    if [ "$rstatus" = "READY" ]; then
      alive_agents+=("$aname:$port")
    fi
  fi
done

if [ ${#alive_agents[@]} -eq 0 ]; then
  log_fail "无 agent 就绪，退出"
  rm -rf "$TMPDIR"
  exit 1
fi

# 阶段 2: 探测角色状态 + 确认（LLM token 消耗提示）
log_info "阶段 2/3: 探测角色状态..."
if [ "$FORCE_REGISTER" = "yes" ]; then
  need_register=0
  for entry in "${alive_agents[@]}"; do
    IFS=':' read -r aname port <<< "$entry"
    need_register=$((need_register + 1))
  done
  if [ $need_register -gt 0 ]; then
    echo ""
    echo -e "${YELLOW}即将为 ${need_register} 个 agent 归隐旧角色并注册新角色（会消耗 LLM token）${NC}"
    echo -ne "${BOLD}确认继续? [Y/n]（30s 后自动继续）${NC} "
    if ! read -r -t 30 confirm 2>/dev/null; then
      confirm=""
    fi
    if [ "$confirm" = "n" ] || [ "$confirm" = "N" ]; then
      log_info "用户取消"
      rm -rf "$TMPDIR"
      exit 0
    fi
  fi
fi

# 阶段 3: 并行处理（server 端归隐 + 生成 + 注册 + 验证）
echo ""
log_info "阶段 3/3: 并行处理角色..."
echo ""

pids=()
for entry in "${AGENTS[@]}"; do
  IFS=':' read -r aname port <<< "$entry"
  if [ -n "$TARGET_AGENT" ] && [ "$aname" != "$TARGET_AGENT" ]; then
    continue
  fi

  if [ ! -f "$TMPDIR/${aname}.ready" ] || [ "$(cut -d'|' -f1 < "$TMPDIR/${aname}.ready")" != "READY" ]; then
    echo "FAIL|$aname|HTTP API 未就绪" > "$TMPDIR/${aname}.result"
    log_fail "[$aname] 跳过（API 未就绪）"
    continue
  fi

  token=$(current_token "$port")
  process_agent "$aname" "$port" "$token" &
  pids+=($!)
done

if [ ${#pids[@]} -gt 0 ]; then
  for pid in "${pids[@]}"; do
    wait "$pid" 2>/dev/null || true
  done
fi

echo ""
echo "=========================================="
echo " 结果汇总"
echo "=========================================="

fail_count=0
ok_count=0
skip_count=0

for entry in "${AGENTS[@]}"; do
  IFS=':' read -r aname port <<< "$entry"
  if [ -n "$TARGET_AGENT" ] && [ "$aname" != "$TARGET_AGENT" ]; then
    continue
  fi

  result_file="$TMPDIR/${aname}.result"
  if [ ! -f "$result_file" ]; then
    log_fail "[$aname] 无结果文件"
    fail_count=$((fail_count + 1))
    continue
  fi

  IFS='|' read -r status agent detail < "$result_file"

  case "$status" in
    OK)
      log_ok "[$agent] $detail"
      ok_count=$((ok_count + 1))
      ;;
    SKIP)
      log_info "[$agent] $detail"
      skip_count=$((skip_count + 1))
      ;;
    FAIL)
      log_fail "[$agent] $detail"
      fail_count=$((fail_count + 1))
      ;;
    *)
      log_fail "[$agent] 未知状态: $status $detail"
      fail_count=$((fail_count + 1))
      ;;
  esac
done

rm -rf "$TMPDIR"

echo ""
total=$((ok_count + skip_count + fail_count))
echo -e " 共 ${total} 个 agent: ${GREEN}${ok_count} 注册并验证${NC} / ${GRAY}${skip_count} 跳过${NC} / ${RED}${fail_count} 失败${NC}"

if [ $fail_count -gt 0 ]; then
  exit 1
fi
