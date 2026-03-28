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
#   restart [--prod] [--no-cache]   重启服务（重建容器 + 可选重新构建镜像）
#   status             查看状态
#   logs               查看日志
#   build [--no-cache] 构建镜像
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

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)" || exit 1

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
prompt() { echo -ne "${CYAN}${1:-}${NC}"; }

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
    local dep_cmd
    for dep_cmd in docker; do
        if ! command -v "$dep_cmd" &> /dev/null; then
            error "$dep_cmd 未安装"
        fi
    done

    if ! docker compose version &> /dev/null 2>&1; then
        error "Docker Compose 未安装"
    fi

    # 检查 Docker 版本并启用 BuildKit（Dockerfile 使用了 BuildKit 缓存挂载）
    local docker_version
    docker_version=$(docker version --format '{{.Server.Version}}' 2>/dev/null | cut -d'.' -f1)
    if [ -n "$docker_version" ] && [ "$docker_version" -lt 23 ]; then
        warn "Docker 版本 < 23.0，建议升级以获得更好的 BuildKit 支持"
        # 为旧版本 Docker 显式启用 BuildKit
        export DOCKER_BUILDKIT=1
        export COMPOSE_DOCKER_CLI_BUILD=1
    fi
}

# ============================================================================
# 生成随机密码
# ============================================================================
generate_random_password() {
    openssl rand -hex 16 2>/dev/null || uuidgen | tr -d '-'
}

# ============================================================================
# 确保数据库密码安全（生产环境）
# ============================================================================
ensure_secure_db_password() {
    local mode="$1"
    local server_dir="$PROJECT_ROOT/crates/server"
    local env_file="$server_dir/.env"

    # # 只在生产环境检查
    # if [ "$mode" != "prod" ]; then
    #     return 0
    # fi

    # 检查 .env 文件是否存在
    if [ ! -f "$env_file" ]; then
        # 从 .env.example 复制
        if [ -f "$server_dir/.env.example" ]; then
            cp "$server_dir/.env.example" "$env_file"
            info "已创建 .env 文件"
        else
            return 0
        fi
    fi

    # 检查当前密码是否为 changeme
    local current_password
    current_password=$(grep -E "^DB_PASSWORD=" "$env_file" 2>/dev/null | cut -d'=' -f2 || echo "changeme")

    if [ "$current_password" = "changeme" ] || [ -z "$current_password" ]; then
        local new_password
        new_password=$(generate_random_password)

        # 更新或添加 DB_PASSWORD
        if grep -q "^DB_PASSWORD=" "$env_file" 2>/dev/null; then
            # macOS 和 Linux 兼容的 sed 语法
            if [[ "$OSTYPE" == "darwin"* ]]; then
                sed -i '' "s/^DB_PASSWORD=.*/DB_PASSWORD=$new_password/" "$env_file"
            else
                sed -i "s/^DB_PASSWORD=.*/DB_PASSWORD=$new_password/" "$env_file"
            fi
        else
            echo "DB_PASSWORD=$new_password" >> "$env_file"
        fi

        # 同时更新 DATABASE_URL（如果存在且包含 changeme）
        if grep -q ":changeme@" "$env_file" 2>/dev/null; then
            if [[ "$OSTYPE" == "darwin"* ]]; then
                sed -i '' "s/:changeme@/:$new_password@/g" "$env_file"
            else
                sed -i "s/:changeme@/:$new_password@/g" "$env_file"
            fi
        fi

        # 保存密码到临时文件供用户查看
        local password_file="$server_dir/cyber_jianghu_db_password.tmp"
        cat > "$password_file" << EOF
========================================
Cyber-Jianghu 数据库密码（已自动生成）
========================================

DB_PASSWORD=$new_password

请妥善保管此密码！
如需手动配置，请更新 crates/server/.env 文件。

========================================
EOF

        success "已为生产环境生成安全的数据库密码"
        info "密码已保存到: $password_file"
        warn "请使用 'cat $password_file' 查看密码"
    fi
}

resolve_mode() {
    local raw_mode="${1:-}"
    case "$raw_mode" in
        ""|dev|--dev)
            printf '%s\n' "dev"
            ;;
        prod|--prod)
            printf '%s\n' "prod"
            ;;
        *)
            return 1
            ;;
    esac
}

enter_component_dir() {
    local component_name="$1"
    local component_dir="$PROJECT_ROOT/crates/$component_name"
    [ -d "$component_dir" ] || error "组件目录不存在: $component_dir"
    cd "$component_dir"
}

ensure_network() {
    local network_name="${1:-}"
    [ -n "$network_name" ] || error "网络名称为空"
    if ! docker network inspect "$network_name" >/dev/null 2>&1; then
        info "创建 Docker 网络: $network_name"
        docker network create "$network_name" >/dev/null
    fi
}

# ============================================================================
# 服务端命令
# ============================================================================
cmd_server_start() {
    local mode
    mode="$(resolve_mode "${1:-}")" || error "无效模式参数: ${1:-} (仅支持 --prod)"
    # 确保生产环境有安全的数据库密码
    ensure_secure_db_password "$mode"
    local compose_file="docker-compose.yml"
    [ "$mode" = "prod" ] && compose_file="docker-compose.prod.yml"
    ensure_network "cyber-jianghu-network"
    enter_component_dir "server"
    info "启动服务端 ($mode)..."
    docker compose -f "$compose_file" up -d

    # 尝试读取 admin token 并拼接到 URL
    local admin_token_file="$PROJECT_ROOT/crates/server/cyber_jianghu_admin.tmp"
    local read_token=""
    local write_token=""
    if [ -f "$admin_token_file" ]; then
        echo ""
        info "管理员令牌已生成，查看: cat $admin_token_file"
        # 提取 Read Token（在 "Read Token" 行的下一行）
        read_token=$(grep -A1 "Read Token" "$admin_token_file" | tail -1 | tr -d ' ')
        # 提取 Write Token（在 "Write Token" 行的下一行）
        write_token=$(grep -A1 "Write Token" "$admin_token_file" | tail -1 | tr -d ' ')
    fi

    # 提示用户查看生成的密码和令牌（如果有）
    local password_file="$PROJECT_ROOT/crates/server/cyber_jianghu_db_password.tmp"
    if [ -f "$password_file" ]; then
        echo ""
        info "数据库密码已自动生成，查看: cat $password_file"
    fi

    success "服务端已启动"
    echo ""
    info "访问地址:"
    if [ -n "$read_token" ]; then
        echo "  - Dashboard (只读): http://localhost:23333/admin?token=$read_token"
    else
        echo "  - Dashboard (只读): http://localhost:23333/admin"
    fi
    if [ -n "$write_token" ]; then
        echo "  - Dashboard (读写): http://localhost:23333/admin?token=$write_token"
    fi
    echo "  - WebSocket: ws://localhost:23333/ws"
    echo "  - Health:    http://localhost:23333/health"
}

cmd_server_stop() {
    enter_component_dir "server"
    info "停止服务端..."
    docker compose down
    success "服务端已停止"
}
cmd_server_restart() {
    local mode=""
    local no_cache=""
    for arg in "$1" "$2"; do
        [ "$arg" = "--prod" ] && mode="prod"
        [ "$arg" = "--no-cache" ] && no_cache="yes"
    done
    ensure_network "cyber-jianghu-network"
    enter_component_dir "server"
    info "重建服务端镜像并重启..."
    local compose_file="docker-compose.yml"
    [ "$mode" = "prod" ] && compose_file="docker-compose.prod.yml"
    [ -n "$no_cache" ] && docker compose -f "$compose_file" build --no-cache || docker compose -f "$compose_file" build
    docker compose -f "$compose_file" up -d --force-recreate
    success "服务端已重启"
}
cmd_server_status() {
    enter_component_dir "server"
    info "服务端状态:"
    docker compose ps
}
cmd_server_logs() {
    enter_component_dir "server"
    docker compose logs -f
}
cmd_server_build() {
    local no_cache="${1:-}"
    enter_component_dir "server"
    info "构建服务端镜像..."
    local build_args=""
    [ "$no_cache" = "--no-cache" ] && build_args="--no-cache"
    docker compose build $build_args
    success "构建完成"
}
cmd_server_reset() {
    enter_component_dir "server"
    warn "将删除所有数据！"
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
    local mode
    mode="$(resolve_mode "${1:-}")" || error "无效模式参数: ${1:-} (仅支持 --prod)"
    local compose_file="docker-compose.yml"
    [ "$mode" = "prod" ] && compose_file="docker-compose.prod.yml"
    ensure_network "cyber-jianghu-network"
    enter_component_dir "agent"
    local agent_port="${AGENT_PORT:-23340}"
    info "启动 Agent ($mode)..."
    SERVER_WS_URL="ws://cyber-jianghu-server:23333/ws" \
    SERVER_HTTP_URL="http://cyber-jianghu-server:23333" \
    AGENT_PORT="$agent_port" \
    docker compose -f "$compose_file" up -d
    success "Agent 已启动"
    echo ""
    info "访问地址:"
    echo "  - Web Panel:  http://localhost:${agent_port}/welcome.html"
    echo "  - HTTP API:   http://localhost:${agent_port}/api/v1"
    echo "  - Health:     http://localhost:${agent_port}/api/v1/health"
}
cmd_agent_stop() {
    ensure_network "cyber-jianghu-network"
    enter_component_dir "agent"
    info "停止 Agent..."
    docker compose down
    success "Agent 已停止"
}
cmd_agent_restart() {
    local mode=""
    local no_cache=""
    for arg in "$1" "$2"; do
        [ "$arg" = "--prod" ] && mode="prod"
        [ "$arg" = "--no-cache" ] && no_cache="yes"
    done
    ensure_network "cyber-jianghu-network"
    enter_component_dir "agent"
    info "重建 Agent 镜像并重启..."
    local compose_file="docker-compose.yml"
    [ "$mode" = "prod" ] && compose_file="docker-compose.prod.yml"
    [ -n "$no_cache" ] && docker compose -f "$compose_file" build --no-cache || docker compose -f "$compose_file" build
    docker compose -f "$compose_file" up -d --force-recreate
    success "Agent 已重启"
}
cmd_agent_status() {
    ensure_network "cyber-jianghu-network"
    enter_component_dir "agent"
    info "Agent 状态:"
    docker compose ps
}
cmd_agent_logs() {
    ensure_network "cyber-jianghu-network"
    enter_component_dir "agent"
    docker compose logs -f
}
cmd_agent_build() {
    local no_cache="${1:-}"
    ensure_network "cyber-jianghu-network"
    enter_component_dir "agent"
    info "构建 Agent 镜像..."
    local build_args=""
    [ "$no_cache" = "--no-cache" ] && build_args="--no-cache"
    docker compose build $build_args
    success "构建完成"
}
cmd_agent_reset() {
    ensure_network "cyber-jianghu-network"
    enter_component_dir "agent"
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
    local mode
    mode="$(resolve_mode "${1:-}")" || error "无效模式参数: ${1:-} (仅支持 --prod)"
    show_banner
    # 确保生产环境有安全的数据库密码（在启动服务前）
    ensure_secure_db_password "$mode"
    cmd_server_start "$mode"
    echo ""
    cmd_agent_start "$mode"
    # 提示用户查看生成的密码（如果有）
    local password_file="$PROJECT_ROOT/crates/server/cyber_jianghu_db_password.tmp"
    if [ -f "$password_file" ]; then
        echo ""
        info "数据库密码已自动生成，查看: cat $password_file"
    fi
}
cmd_all_stop() {
    cmd_server_stop
    cmd_agent_stop
}
cmd_all_restart() {
    local mode=""
    local no_cache=""
    for arg in "$1" "$2"; do
        [ "$arg" = "--prod" ] && mode="prod"
        [ "$arg" = "--no-cache" ] && no_cache="yes"
    done
    cmd_server_restart "$mode" "$no_cache"
    cmd_agent_restart "$mode" "$no_cache"
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
    local no_cache="${1:-}"
    cmd_server_build "$no_cache"
    cmd_agent_build "$no_cache"
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
    echo "  restart [--prod] [--no-cache]   重启服务"
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
    echo "  $0 all build --no-cache   # 强制重新构建（忽略缓存）"
    echo "  $0 server restart --no-cache  # 重建服务端容器 + 重新构建镜像"
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

    case "$cmd" in
        start|stop|restart|status|logs|build|reset)
            check_dependencies
            ;;
    esac

    # 分发到对应组件的处理函数
    case "$component" in
        server)
            case "$cmd" in
                start)     cmd_server_start "${3:-}" ;;
                stop)       cmd_server_stop ;;
                restart)    cmd_server_restart "${3:-}" "${4:-}" ;;
                status)     cmd_server_status ;;
                logs)       cmd_server_logs ;;
                build)      cmd_server_build "${3:-}" ;;
                reset)      cmd_server_reset ;;
                *)          error "未知命令: $cmd" ;;
            esac
            ;;
        agent)
            case "$cmd" in
                start)     cmd_agent_start "${3:-}" ;;
                stop)       cmd_agent_stop ;;
                restart)    cmd_agent_restart "${3:-}" "${4:-}" ;;
                status)     cmd_agent_status ;;
                logs)       cmd_agent_logs ;;
                build)      cmd_agent_build "${3:-}" ;;
                reset)      cmd_agent_reset ;;
                *)          error "未知命令: $cmd" ;;
            esac
            ;;
        all)
            case "$cmd" in
                start)     cmd_all_start "${3:-}" ;;
                stop)       cmd_all_stop ;;
                restart)    cmd_all_restart "${3:-}" "${4:-}" ;;
                status)     cmd_all_status ;;
                logs)       cmd_all_logs ;;
                build)      cmd_all_build "${3:-}" ;;
                reset)      cmd_all_reset ;;
                *)          error "未知命令: $cmd" ;;
            esac
            ;;
        *)
            error "未知组件: $component"
            ;;
    esac
}

main "$@"
