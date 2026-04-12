#!/bin/bash
# ============================================================================
# Cyber-Jianghu 安装部署脚本
# ============================================================================

set -eu

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
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

info()    { echo -e "${BLUE}[INFO]${NC} ${1:-}"; }
success() { echo -e "${GREEN}[OK]${NC} ${1:-}"; }
warn()    { echo -e "${YELLOW}[WARN]${NC} ${1:-}"; }
error()   { echo -e "${RED}[ERROR]${NC} ${1:-}"; exit 1; }
prompt()  { echo -ne "${CYAN}${1:-}${NC}"; }

# ============================================================================
# Banner
# ============================================================================
show_banner() {
    echo -e "${GREEN}
╔══════════════════════════════════════════════════════════════╗
║     Cyber-Jianghu (赛博江湖)                        ║
║     天道无为，万物自化                              ║
╚══════════════════════════════════════════════════════════════╝${NC}"
}

# ============================================================================
# 依赖检查
# ============================================================================
check_dependencies() {
    command -v docker &>/dev/null || error "docker 未安装"
    docker compose version &>/dev/null || error "Docker Compose 未安装"
}

# ============================================================================
# 跨平台 sed（macOS 需要空后缀，Linux 不需要）
# ============================================================================
sed_i() {
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "$@" 2>/dev/null || true
    else
        sed -i "$@" 2>/dev/null || true
    fi
}

# ============================================================================
# 生成随机密码
# ============================================================================
generate_random_password() {
    openssl rand -hex 16 2>/dev/null || uuidgen | tr -d '-'
}

# ============================================================================
# 确保数据库密码安全
# ============================================================================
ensure_secure_db_password() {
    local env_file="$PROJECT_ROOT/crates/server/.env"

    if [ ! -f "$env_file" ]; then
        [ -f "$env_file.example" ] && cp "$env_file.example" "$env_file" || return 0
        info "已创建 .env 文件"
    fi

    local current_password
    current_password=$(grep -E "^DB_PASSWORD=" "$env_file" 2>/dev/null | cut -d'=' -f2 || echo "changeme")
    [ "$current_password" != "changeme" ] && [ -n "$current_password" ] && return 0

    local new_password
    new_password=$(generate_random_password)

    if grep -q "^DB_PASSWORD=" "$env_file" 2>/dev/null; then
        sed_i "s/^DB_PASSWORD=.*/DB_PASSWORD=$new_password/" "$env_file"
    else
        echo "DB_PASSWORD=$new_password" >> "$env_file"
    fi

    sed_i "s/:changeme@/:$new_password@/g" "$env_file"

    local password_file="$PROJECT_ROOT/crates/server/cyber_jianghu_db_password.tmp"
    cat > "$password_file" << EOF
========================================
Cyber-Jianghu 数据库密码（已自动生成）
========================================
DB_PASSWORD=$new_password
请妥善保管此密码！
========================================
EOF

    success "已生成安全的数据库密码"
    info "密码已保存到: $password_file"
}

# ============================================================================
# 解析模式参数
# ============================================================================
resolve_mode() {
    case "${1:-}" in
        --prod|prod) echo "prod" ;;
        *)            echo "dev"  ;;
    esac
}

# ============================================================================
# 网络确保
# ============================================================================
ensure_network() {
    docker network inspect "$1" &>/dev/null || docker network create "$1" >/dev/null
}

# ============================================================================
# 通用组件启动
# ============================================================================
cmd_component_start() {
    local component="$1"
    local mode
    mode="$(resolve_mode "${2:-}")"

    local compose_file="docker-compose.yml"
    [ "$mode" = "prod" ] && compose_file="docker-compose.prod.yml"

    ensure_network "cyber-jianghu-network"
    cd "$PROJECT_ROOT/crates/$component"

    case "$component" in
        server)
            ensure_secure_db_password
            info "启动服务端 ($mode)..."
            docker compose -f "$compose_file" up -d

            local admin_token_file="$PROJECT_ROOT/crates/server/logs/cyber_jianghu_admin.tmp"
            local read_token="" write_token=""
            [ -f "$admin_token_file" ] && {
                read_token=$(grep -A1 "Read Token" "$admin_token_file" | tail -1 | tr -d ' ')
                write_token=$(grep -A1 "Write Token" "$admin_token_file" | tail -1 | tr -d ' ')
            }

            success "服务端已启动"
            echo ""
            info "访问地址:"
            echo "  - Dashboard: http://localhost:23333/admin${read_token:+?token=$read_token}"
            echo "  - WebSocket: ws://localhost:23333/ws"
            echo "  - Health:    http://localhost:23333/health"
            ;;
        agent)
            info "启动 Agent ($mode)..."
            local agent_port="${AGENT_PORT:-23340}"
            local agent_env_file="$PROJECT_ROOT/crates/agent/.env.agent"
            cat > "$agent_env_file" << EOF
SERVER_WS_URL=ws://cyber-jianghu-server:23333/ws
SERVER_HTTP_URL=http://cyber-jianghu-server:23333
AGENT_PORT=$agent_port
EOF
            docker compose -f "$compose_file" --env-file "$agent_env_file" up -d
            success "Agent 已启动"
            echo ""
            info "访问地址:"
            echo "  - Web Panel: http://localhost:${agent_port}/welcome.html"
            echo "  - HTTP API:  http://localhost:${agent_port}/api/v1"
            ;;
    esac
}

# ============================================================================
# 通用组件停止
# ============================================================================
cmd_component_stop() {
    cd "$PROJECT_ROOT/crates/$1"
    info "停止 $1..."
    docker compose down
    success "$1 已停止"
}

# ============================================================================
# 通用组件重启
# ============================================================================
cmd_component_restart() {
    local component="$1"; shift
    local mode="dev" no_cache=""
    for arg in "$@"; do
        case "$arg" in
            --prod) mode="prod" ;;
            --no-cache) no_cache="--no-cache" ;;
        esac
    done
    cmd_component_stop "$component"
    cmd_component_build "$component" "$no_cache"
    cmd_component_start "$component" "$mode"
}

# ============================================================================
# 通用组件状态
# ============================================================================
cmd_component_status() {
    cd "$PROJECT_ROOT/crates/$1"
    info "$1 状态:"
    docker compose ps
}

# ============================================================================
# 通用组件日志
# ============================================================================
cmd_component_logs() {
    cd "$PROJECT_ROOT/crates/$1"
    docker compose logs -f
}

# ============================================================================
# 通用组件构建
# ============================================================================
cmd_component_build() {
    local component="$1"; shift || true
    cd "$PROJECT_ROOT/crates/$component"
    info "构建 $component 镜像..."
    [ "${1:-}" = "--no-cache" ] && docker compose build --no-cache || docker compose build
    success "构建完成"
}

# ============================================================================
# 通用组件重置
# ============================================================================
cmd_component_reset() {
    cd "$PROJECT_ROOT/crates/$1"
    warn "将删除 $1 所有数据！"
    prompt "确认重置? (y/N): "
    read -r confirm
    [ "$confirm" = "y" ] || exit 0
    docker compose down -v
    success "$1 数据已重置"
}

# ============================================================================
# 全部组件命令
# ============================================================================
cmd_all_start() {
    show_banner
    cmd_component_start server "${1:-}"
    echo ""
    cmd_component_start agent "${1:-}"
}

cmd_all_stop()    { cmd_component_stop server; cmd_component_stop agent; }
cmd_all_restart() { cmd_component_restart server "$@"; cmd_component_restart agent "$@"; }

cmd_all_status() {
    echo "=== 服务端 ==="; cmd_component_status server
    echo ""; echo "=== Agent ==="; cmd_component_status agent
}

cmd_all_logs() {
    echo "=== 服务端日志 ==="
    cmd_component_logs server &
    local server_pid=$!
    echo ""; echo "=== Agent 日志 ==="
    cmd_component_logs agent
    kill $server_pid 2>/dev/null
}

cmd_all_build() { cmd_component_build server "$@"; cmd_component_build agent "$@"; }

cmd_all_reset() {
    warn "将删除所有数据！"
    prompt "确认重置所有数据? (y/N): "
    read -r confirm
    [ "$confirm" = "y" ] || exit 0
    cmd_component_reset server
    cmd_component_reset agent
}

# ============================================================================
# 帮助信息
# ============================================================================
show_help() {
    show_banner
    echo "用法: $0 <component> <command> [参数]"
    echo ""
    echo -e "${GREEN}组件:${NC}  server | agent | all"
    echo -e "${GREEN}命令:${NC}  start [--prod] | stop | restart [--prod] [--no-cache] | status | logs | build [--no-cache] | reset"
    echo ""
    echo -e "${GREEN}示例:${NC}"
    echo "  $0 server start --prod"
    echo "  $0 agent restart --no-cache"
    echo "  $0 all start"
}

# ============================================================================
# 主入口
# ============================================================================
main() {
    local component="${1:-}"
    local cmd="${2:-}"

    [ -z "$component" ] || [ -z "$cmd" ] && { show_help; exit 1; }

    case "$cmd" in
        start|stop|restart|status|logs|build|reset) check_dependencies ;;
    esac

    case "$component" in
        server|agent)
            case "$cmd" in
                start)    cmd_component_start "$component" "${3:-}" ;;
                stop)     cmd_component_stop "$component" ;;
                restart)  cmd_component_restart "$component" "${@:3}" ;;
                status)   cmd_component_status "$component" ;;
                logs)     cmd_component_logs "$component" ;;
                build)    cmd_component_build "$component" "${@:3}" ;;
                reset)    cmd_component_reset "$component" ;;
                *)        error "未知命令: $cmd" ;;
            esac
            ;;
        all)
            case "$cmd" in
                start)    cmd_all_start "${3:-}" ;;
                stop)     cmd_all_stop ;;
                restart)  cmd_all_restart "${@:3}" ;;
                status)   cmd_all_status ;;
                logs)     cmd_all_logs ;;
                build)    cmd_all_build "${@:3}" ;;
                reset)    cmd_all_reset ;;
                *)        error "未知命令: $cmd" ;;
            esac
            ;;
        *)  error "未知组件: $component" ;;
    esac
}

main "$@"
