#!/bin/bash
# restart.sh - 快速重启 .test-agents 下的 Docker agent，并行检测存活角色并注册
#
# 用法:
#   ./restart.sh              # 重启所有 agent
#   ./restart.sh agent-1      # 只重启指定 agent
#   ./restart.sh --register   # 重启 + 强制注册（存活角色先归隐再注册）
#   ./restart.sh --no-register # 重启 + 跳过注册
#   ./restart.sh --build      # 重启前重新构建镜像

set -uo pipefail
cd "$(dirname "$0")"

# ── 配置 ──────────────────────────────────────────────────────────────────────
AGENTS=(
  "agent-1:test-agent-1:23341"
  "agent-2:test-agent-2:23342"
  "agent-3:test-agent-3:23343"
  "agent-5:test-agent-5:23345"
  "agent-6:test-agent-6:23346"
  "agent-ollama:test-agent-ollama:23349"
)

READY_TIMEOUT=30
RETIRE_POLL_INTERVAL=2
RETIRE_POLL_MAX=15
GEN_RETRY_MAX=4
TMPDIR="./tmp/restart_$$"

# 信号清理：Ctrl+C 时删除临时目录（TMPDIR 必须先赋值）
trap 'rm -rf "$TMPDIR" 2>/dev/null; exit 130' INT TERM

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
      echo "  --register      强制注册（存活角色先归隐再注册新角色）"
      echo "  --no-register   跳过注册"
      echo "  --build         重启前重新构建镜像"
      echo "  agent-name      只操作指定 agent（如 agent-1）"
      exit 0
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
log_warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }

emit() {
  local aname=$1 phase=$2 msg=$3
  local ts
  ts=$(date '+%H:%M:%S')
  echo -e "${GRAY}${ts}${NC} ${BOLD}[${aname}]${NC} ${phase} ${msg}"
}

# ── 函数 ──────────────────────────────────────────────────────────────────────

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

# 等待 rebirth 清理完成（轮询 has_character 变为 false）
wait_for_retire() {
  local port=$1 aname=$2
  local attempts=0
  while [ $attempts -lt $RETIRE_POLL_MAX ]; do
    local check
    check=$(curl -sf "http://localhost:${port}/api/v1/setup/status" --max-time 3 2>/dev/null || true)
    if [ -n "$check" ]; then
      local hc
      hc=$(echo "$check" | python3 -c "import sys,json; d=json.load(sys.stdin); print(str(d.get('has_character',True)).lower())" 2>/dev/null || true)
      if [ "$hc" = "false" ]; then
        return 0
      fi
    fi
    sleep "$RETIRE_POLL_INTERVAL"
    attempts=$((attempts + 1))
  done
  return 1
}

# 调用 agent rebirth 端点归隐当前角色（幂等）
do_retire() {
  local port=$1 aname=$2
  curl -sf -X POST "http://localhost:${port}/api/v1/character/rebirth" \
    -H "Content-Type: application/json" \
    -d '{"confirm": true}' \
    --max-time 30 2>/dev/null || true
}

# 生成 + 注册新角色（LLM 生成自带重试）
do_register() {
  local port=$1 aname=$2
  local gen_file="$TMPDIR/${aname}.gen.json"

  local gen_ok=false
  for attempt in $(seq 1 $((GEN_RETRY_MAX + 1))); do
    emit "$aname" "${CYAN}LLM${NC}" "正在生成角色（尝试 ${attempt}/$((GEN_RETRY_MAX + 1))，最长 180s）..."
    if curl -sf -X POST "http://localhost:${port}/api/v1/character/generate" \
      -H "Content-Type: application/json" \
      -d '{}' \
      -o "$gen_file" \
      --max-time 180 2>/dev/null && \
       [ -s "$gen_file" ] && \
       python3 -c "import json; json.load(open('${gen_file}'))" 2>/dev/null; then
      gen_ok=true
      break
    fi
    [ $attempt -le $GEN_RETRY_MAX ] && emit "$aname" "${YELLOW}RETRY${NC}" "生成失败，${RETIRE_POLL_INTERVAL}s 后重试..."
    sleep "$RETIRE_POLL_INTERVAL"
  done

  if [ "$gen_ok" = false ]; then
    echo "FAIL|$aname|角色生成失败（已重试 ${GEN_RETRY_MAX} 次）" > "$TMPDIR/${aname}.result"
    emit "$aname" "${RED}FAIL${NC}" "角色生成失败（已重试 ${GEN_RETRY_MAX} 次）"
    return 1
  fi

  local gen_name
  gen_name=$(python3 -c "import json; print(json.load(open('${gen_file}')).get('name','?'))" 2>/dev/null || true)
  emit "$aname" "${GREEN}GEN${NC}" "生成角色: ${gen_name}"

  emit "$aname" "${CYAN}REG${NC}" "注册到服务器..."
  local reg_result
  reg_result=$(curl -sf -X POST "http://localhost:${port}/api/v1/character/register" \
    -H "Content-Type: application/json" \
    -d @"$gen_file" \
    --max-time 30 2>/dev/null || true)

  if [ -z "$reg_result" ]; then
    echo "FAIL|$aname|角色注册失败（无响应）" > "$TMPDIR/${aname}.result"
    emit "$aname" "${RED}FAIL${NC}" "角色注册失败（无响应）"
    return 1
  fi

  local reg_msg
  reg_msg=$(echo "$reg_result" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    print(d.get('message', '???'))
except: print('???')" 2>/dev/null || true)
  echo "OK|$aname|$reg_msg" > "$TMPDIR/${aname}.result"
  emit "$aname" "${GREEN}DONE${NC}" "注册成功: ${reg_msg}"
  return 0
}

# 单个 agent 完整流程（调用前 API 已确认就绪）
process_agent() {
  local aname=$1 port=$2
  local outfile="$TMPDIR/${aname}.result"

  if [ "$FORCE_REGISTER" = "no" ]; then
    echo "SKIP|$aname|跳过注册" > "$outfile"
    emit "$aname" "${GRAY}SKIP${NC}" "跳过注册"
    return
  fi

  # 检查角色状态
  local setup_json
  setup_json=$(curl -sf "http://localhost:${port}/api/v1/setup/status" --max-time 5 2>/dev/null || true)

  if [ -z "$setup_json" ]; then
    echo "FAIL|$aname|无法获取 setup status" > "$outfile"
    emit "$aname" "${RED}FAIL${NC}" "无法获取 setup status"
    return
  fi

  local is_dead has_char
  is_dead=$(echo "$setup_json" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    print(str(d.get('is_dead', False)).lower())
except: print('error')" 2>/dev/null)
  has_char=$(echo "$setup_json" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    print(str(d.get('has_character', False)).lower())
except: print('error')" 2>/dev/null)

  # 按状态决定路径
  if [ "$has_char" = "true" ] && [ "$is_dead" = "false" ]; then
    local char_name
    char_name=$(curl -sf "http://localhost:${port}/api/v1/character" --max-time 5 2>/dev/null \
      | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('name','?'))" 2>/dev/null || true)

    if [ "$FORCE_REGISTER" != "yes" ]; then
      echo "ALIVE|$aname|$char_name" > "$outfile"
      emit "$aname" "${GREEN}ALIVE${NC}" "已有存活角色「${char_name}」"
      return
    fi

    # --register: 先归隐，轮询等待清理完成，再注册
    emit "$aname" "${YELLOW}RETIRE${NC}" "归隐存活角色「${char_name}」..."
    local retire_result
    retire_result=$(do_retire "$port" "$aname")

    if [ -z "$retire_result" ]; then
      echo "FAIL|$aname|归隐失败（无响应）" > "$outfile"
      emit "$aname" "${RED}FAIL${NC}" "归隐失败（无响应）"
      return
    fi

    emit "$aname" "${CYAN}WAIT${NC}" "等待 rebirth 清理..."
    if ! wait_for_retire "$port" "$aname"; then
      echo "FAIL|$aname|rebirth 清理超时" > "$outfile"
      emit "$aname" "${RED}FAIL${NC}" "rebirth 清理超时（${RETIRE_POLL_MAX} 次轮询）"
      return
    fi
    emit "$aname" "${GREEN}RETIRE-OK${NC}" "归隐完成"

    do_register "$port" "$aname"
    return
  fi

  # dead 或 none → 直接注册
  if [ "$is_dead" = "true" ]; then
    emit "$aname" "${YELLOW}DEAD${NC}" "角色已死亡，需要注册新角色"
  else
    emit "$aname" "${CYAN}NONE${NC}" "无角色，需要注册"
  fi

  do_register "$port" "$aname"
}

# ── 主流程 ────────────────────────────────────────────────────────────────────

echo "=========================================="
echo " Cyber-Jianghu Agent Restart Tool"
echo "=========================================="
echo ""

AGENT_COMPOSE="../crates/agent/docker-compose.yml"

if [ "$DO_BUILD" = true ]; then
  log_info "构建 agent 镜像（使用 $AGENT_COMPOSE）..."
  docker compose -f "$AGENT_COMPOSE" build 2>&1 | tail -5
  log_ok "构建完成，镜像 agent-agent:latest 已更新"
fi

log_info "重启 Docker 容器..."
if [ -n "$TARGET_AGENT" ]; then
  found=false
  for entry in "${AGENTS[@]}"; do
    IFS=':' read -r aname _ _ <<< "$entry"
    if [ "$aname" = "$TARGET_AGENT" ]; then
      found=true
      break
    fi
  done
  if [ "$found" = false ]; then
    log_fail "未知 agent: $TARGET_AGENT"
    echo "可用: $(for e in "${AGENTS[@]}"; do IFS=':' read -r n _ _ <<< "$e"; echo -n "$n "; done)"
    exit 1
  fi
  docker compose -f docker-compose.yml restart "$TARGET_AGENT"
else
  docker compose -f docker-compose.yml restart
fi
log_ok "容器已重启"

echo ""
mkdir -p "$TMPDIR"

# 阶段 1: 并行等待所有 agent API 就绪
log_info "阶段 1/3: 等待所有 agent API 就绪..."
ready_pids=()
for entry in "${AGENTS[@]}"; do
  IFS=':' read -r aname _ port <<< "$entry"
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
  IFS=':' read -r aname _ port <<< "$entry"
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

# 阶段 2: 探测角色状态 + 确认
log_info "阶段 2/3: 探测角色状态..."
if [ "$FORCE_REGISTER" != "no" ]; then
  need_register=0
  dead_list=""
  none_list=""
  alive_list=""
  for entry in "${alive_agents[@]}"; do
    IFS=':' read -r aname port <<< "$entry"
    quick_check=$(curl -sf "http://localhost:${port}/api/v1/setup/status" --max-time 5 2>/dev/null || true)
    if [ -n "$quick_check" ]; then
      q_dead=$(echo "$quick_check" | python3 -c "import sys,json; d=json.load(sys.stdin); print(str(d.get('is_dead',False)).lower())" 2>/dev/null || true)
      q_char=$(echo "$quick_check" | python3 -c "import sys,json; d=json.load(sys.stdin); print(str(d.get('has_character',False)).lower())" 2>/dev/null || true)
      if [ "$q_dead" = "true" ]; then
        need_register=$((need_register + 1))
        dead_list="${dead_list} ${aname}"
      elif [ "$q_char" != "true" ]; then
        need_register=$((need_register + 1))
        none_list="${none_list} ${aname}"
      else
        # 存活角色：仅 --register 模式需要归隐+重注册
        if [ "$FORCE_REGISTER" = "yes" ]; then
          need_register=$((need_register + 1))
          alive_list="${alive_list} ${aname}"
        fi
      fi
    fi
  done

  if [ $need_register -gt 0 ]; then
    echo ""
    echo -e "${YELLOW}即将处理 ${need_register} 个角色（会消耗 LLM token）${NC}"
    [ -n "$alive_list" ] && echo -e "  ${YELLOW}需归隐:${NC}${alive_list}"
    [ -n "$dead_list" ] && echo -e "  ${RED}已死亡:${NC}${dead_list}"
    [ -n "$none_list" ] && echo -e "  ${CYAN}无角色:${NC}${none_list}"
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

# 阶段 3: 并行处理（注册/跳过）
echo ""
log_info "阶段 3/3: 并行处理角色..."
echo ""

pids=()
for entry in "${AGENTS[@]}"; do
  IFS=':' read -r aname _ port <<< "$entry"
  if [ -n "$TARGET_AGENT" ] && [ "$aname" != "$TARGET_AGENT" ]; then
    continue
  fi

  # 未就绪的 agent 直接写 FAIL
  if [ ! -f "$TMPDIR/${aname}.ready" ] || [ "$(cut -d'|' -f1 < "$TMPDIR/${aname}.ready")" != "READY" ]; then
    echo "FAIL|$aname|HTTP API 未就绪" > "$TMPDIR/${aname}.result"
    log_fail "[$aname] 跳过（API 未就绪）"
    continue
  fi

  process_agent "$aname" "$port" &
  pids+=($!)
done

for pid in "${pids[@]}"; do
  wait "$pid" 2>/dev/null || true
done

echo ""
echo "=========================================="
echo " 结果汇总"
echo "=========================================="

fail_count=0
ok_count=0
skip_count=0
alive_count=0

for entry in "${AGENTS[@]}"; do
  IFS=':' read -r aname _ _ <<< "$entry"
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
      log_ok "[$agent] 注册成功: $detail"
      ok_count=$((ok_count + 1))
      ;;
    ALIVE)
      log_ok "[$agent] 已有存活角色: $detail"
      alive_count=$((alive_count + 1))
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
total=$((ok_count + alive_count + skip_count + fail_count))
echo -e " 共 ${total} 个 agent: ${GREEN}${ok_count} 注册${NC} / ${GREEN}${alive_count} 存活${NC} / ${GRAY}${skip_count} 跳过${NC} / ${RED}${fail_count} 失败${NC}"

if [ $fail_count -gt 0 ]; then
  exit 1
fi
