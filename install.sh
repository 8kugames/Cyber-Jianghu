#!/bin/bash
# ============================================================================
# Cyber-Jianghu 安装部署脚本
# ============================================================================
#
# 用法: ./install.sh <component> [command] [参数]
#
# 组件:
#   server    游戏服务端
#   agent    AI 代理
#   all      全部组件（默认）
#
# 命令:
#   start [--prod]     启动服务
#   stop               停止服务
#   restart            重启服务
#   status             查看状态
#   logs               查看日志
#   build             构建镜像
#   reset              重置数据（慎用）
#
# 示例:
#   ./install.sh server start           # 开发环境启动服务端
#   ./install.sh server start --prod    # 生产环境启动服务端
#   ./install.sh agent start            # 开发环境启动 Agent
#   ./install.sh agent start --prod     # 生产环境启动 Agent
#   ./install.sh all start              # 开发环境启动全部
#   ./install.sh all start --prod       # 生产环境启动全部
#
# ============================================================================

set -eu

(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" >/dev/null || exit 1

IFS=$'\n\t'

# ============================================================================
# 颜色输出
# ============================================================================
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

info() { echo -e "${BLUE}[INFO]${NC} ${1:-}"; }
success() { echo -e "${GREEN}[OK]${NC} ${1:-}"; }
warn() { echo -e "${YELLOW}[WARN]${NC} ${1:-}"; }
error() { echo -e "${RED}[ERROR]${NC} ${1:-}"; exit 1; }

# ============================================================================
# 显示 Banner
# ============================================================================
show_banner() {
    echo ""
    echo -e "${GREEN}╔══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${GREEN}║     Cyber-Jianghu (赛博江湖)                        ║${NC}"
    echo -e "${GREEN}║     天道无为，万物自化                              ║${NC}"
    echo -e "${GREEN}╚══════════════════════════════════════════════════════════════╝${NC}"
    echo ""
}

# ============================================================================
# 检查依赖
# ============================================================================
check_dependencies() {
    local missing=0
    for cmd in docker curl; do
        if ! command -v "$cmd" &> /dev/null; then
            error "$cmd 未安装"
            missing=1
        fi
    done

    if ! docker compose version &> /dev/null 2>&1; then
        error "Docker Compose 未安装"
        missing=1
    fi
    [ $missing -eq 1 ] && exit 1
}

# ============================================================================
# 服务端命令
# ============================================================================
cmd_server_start() {
    local mode="${1:-dev}"
    local compose_file="docker-compose.yml"
    [ "$mode" = "prod" ] && compose_file="docker-compose.prod.yml"
    cd "$PROJECT_ROOT/crates/server"
    info "启动服务端 ($mode)..."
    docker compose -f "$compose_file" up -d
    success "服务端已启动"
    echo ""
    info "访问地址:"
    echo "  - Dashboard: http://localhost:23333/admin"
    echo "  - WebSocket: ws://localhost:23333/ws"
    echo "  - Health:    http://localhost:23333/health"
}

cmd_server_stop() {
    cd "$PROJECT_ROOT/crates/server"
    info "停止服务端..."
    docker compose down
    success "服务端已停止"
}
cmd_server_restart() {
    local mode="${1:-dev}"
    cd "$PROJECT_ROOT/crates/server"
    info "重启服务端..."
    local compose_file="docker-compose.yml"
    [ "$mode" = "prod" ] && compose_file="docker-compose.prod.yml"
    docker compose -f "$compose_file" restart
    success "服务端已重启"
}
cmd_server_status() {
    cd "$PROJECT_ROOT/crates/server"
    info "服务端状态:"
    docker compose ps
}
cmd_server_logs() {
    cd "$PROJECT_ROOT/crates/server"
    docker compose logs -f
}
cmd_server_build() {
    cd "$PROJECT_ROOT/crates/server"
    info "构建服务端镜像..."
    docker compose build
    success "构建完成"
}
cmd_server_reset() {
    cd "$PROJECT_ROOT/crates/server"
    warn "将删除所有数据！ 按"
    prompt "确认重置服务端数据? (y/N): "
    read -r confirm
    [ "$confirm" = "y" ] || exit 0
    warn "正在重置..."
    docker compose down -v
    success "服务端数据已重置"
}

# ============================================================================
# Agent 命令
# ============================================================================
cmd_agent_start() {
    local mode="${1:-dev}"
    local compose_file="docker-compose.yml"
    [ "$mode" = "prod" ] && compose_file="docker-compose.prod.yml"
    cd "$PROJECT_ROOT/crates/agent"
    info "启动 Agent ($mode)..."
    docker compose -f "$compose_file" up -d
    success "Agent 已启动"
    echo ""
    info "访问地址:"
    echo "  - HTTP API: http://localhost:23340/api/v1"
    echo "  - Health:  http://localhost:23340/api/v1/health"
}
cmd_agent_stop() {
    cd "$PROJECT_ROOT/crates/agent"
    info "停止 Agent..."
    docker compose down
    success "Agent 已停止"
}
cmd_agent_restart() {
    local mode="${1:-dev}"
    cd "$PROJECT_ROOT/crates/agent"
    info "重启 Agent..."
    local compose_file="docker-compose.yml"
    [ "$mode" = "prod" ] && compose_file="docker-compose.prod.yml"
    docker compose -f "$compose_file" restart
    success "Agent 已重启"
}
cmd_agent_status() {
    cd "$PROJECT_ROOT/crates/agent"
    info "Agent 状态:"
    docker compose ps
}
cmd_agent_logs() {
    cd "$PROJECT_ROOT/crates/agent"
    docker compose logs -f
}
cmd_agent_build() {
    cd "$PROJECT_ROOT/crates/agent"
    info "构建 Agent 镜像..."
    docker compose build
    success "构建完成"
}
cmd_agent_reset() {
    cd "$PROJECT_ROOT/crates/agent"
    warn "将删除所有数据!"
    prompt "确认重置 Agent 数据? (y/N): "
    read -r confirm
    [ "$confirm" = "y" ] || exit 0
    warn "正在重置..."
    docker compose down -v
    success "Agent 数据已重置"
}

# ============================================================================
# 全部组件命令
# ============================================================================
cmd_all_start() {
    local mode="${1:-dev}"
    show_banner
    check_dependencies
    cmd_server_start "$mode"
    echo ""
    cmd_agent_start "$mode"
}
cmd_all_stop() {
    cmd_server_stop
    cmd_agent_stop
}
cmd_all_restart() {
    local mode="${1:-dev}"
    cmd_server_restart "$mode"
    cmd_agent_restart "$mode"
}
cmd_all_status() {
    echo "=== 服务端 ==="
    cmd_server_status
    echo ""
    echo "=== Agent ==="
    cmd_agent_status
}
cmd_all_logs() {
    echo "=== 服务端日志 ==="
    cmd_server_logs &
    echo ""
    echo "=== Agent 日志 ==="
    cmd_agent_logs
}
cmd_all_build() {
    cmd_server_build
    cmd_agent_build
}
cmd_all_reset() {
    warn "将删除所有数据（服务端 + Agent）!"
    prompt "确认重置所有数据? (y/N): "
    read -r confirm
    [ "$confirm" = "y" ] || exit 0
    cmd_server_reset
    cmd_agent_reset
}

# ============================================================================
# 显示帮助
# ============================================================================
show_help() {
    show_banner
    echo "用法: $0 <component> <command> [参数]"
    echo ""
    echo -e "${GREEN}组件:${NC}"
    echo "  server    游戏服务端"
    echo "  agent    AI 代理"
    echo "  all      全部组件（默认）"
    echo ""
    echo -e "${GREEN}命令:${NC}"
    echo "  start [--prod]     启动服务"
    echo "  stop               停止服务"
    echo "  restart [--prod]   重启服务"
    echo "  status             查看状态"
    echo "  logs               查看日志"
    echo "  build             构建镜像"
    echo "  reset              重置数据（慎用）"
    echo ""
    echo -e "${GREEN}示例:${NC}"
    echo "  $0 server start           # 开发环境启动服务端"
    echo "  $0 server start --prod    # 生产环境启动服务端"
    echo "  $0 agent start            # 开发环境启动 Agent"
    echo "  $0 agent start --prod     # 生产环境启动 Agent"
    echo "  $0 all start              # 开发环境启动全部"
    echo "  $0 all start --prod       # 生产环境启动全部"
    echo ""
}

# ============================================================================
# 主入口
# ============================================================================
main() {
    local component="${1:-all}"
    local cmd="${2:-}"

    # 验证参数
    [ -z "$component" ] && { show_help; exit 1; }
    [ -z "$cmd" ] && { show_help; exit 1; }

    # 分发到对应组件的处理函数
    case "$component" in
        server)
            case "$cmd" in
                start)     cmd_server_start "${3:-}" ;;
                stop)       cmd_server_stop ;;
                restart)    cmd_server_restart "${3:-}" ;;
                status)     cmd_server_status ;;
                logs)       cmd_server_logs ;;
                build)      cmd_server_build ;;
                reset)      cmd_server_reset ;;
                *)          error "未知命令: $cmd" ;;
            esac
            ;;
        agent)
            case "$cmd" in
                start)     cmd_agent_start "${3:-}" ;;
                stop)       cmd_agent_stop ;;
                restart)    cmd_agent_restart "${3:-}" ;;
                status)     cmd_agent_status ;;
                logs)       cmd_agent_logs ;;
                build)      cmd_agent_build ;;
                reset)      cmd_agent_reset ;;
                *)          error "未知命令: $cmd" ;;
            esac
            ;;
        all)
            case "$cmd" in
                start)     cmd_all_start "${3:-}" ;;
                stop)       cmd_all_stop ;;
                restart)    cmd_all_restart "${3:-}" ;;
                status)     cmd_all_status ;;
                logs)       cmd_all_logs ;;
                build)      cmd_all_build ;;
                reset)      cmd_all_reset ;;
                *)          error "未知命令: $cmd" ;;
            esac
            ;;
    esac
}
