#!/bin/bash
# ============================================================================
# Cyber-Jianghu 统一管理脚本
# ============================================================================
#
# 用法: ./scripts/cyber-jianghu.sh <命令> [参数]
#
# 命令:
#   start [--prod]     启动服务 (--prod 使用生产配置)
#   stop               停止服务
#   restart            重启服务
#   status             查看状态
#   logs               查看日志
#   build [--release]  构建 Docker 镜像或本地代码
#   test               运行测试
#   reset              重置所有数据
#
# ============================================================================

set -eu
(set -o pipefail) 2>/dev/null && set -o pipefail
IFS=$'\n\t'

# ============================================================================
# 基础配置
# ============================================================================

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVER_URL="${SERVER_URL:-http://localhost:23333}"

# ============================================================================
# 工具函数
# ============================================================================

info() { echo -e "${BLUE}[INFO]${NC} ${1:-}"; }
success() { echo -e "${GREEN}[OK]${NC} ${1:-}"; }
warn() { echo -e "${YELLOW}[WARN]${NC} ${1:-}"; }
error() { echo -e "${RED}[ERROR]${NC} ${1:-}"; exit 1; }
prompt() { echo -ne "${CYAN}[?]${NC} ${1:-}"; }

show_banner() {
    echo ""
    echo -e "${GREEN}╔══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${GREEN}║     Cyber-Jianghu - 天道无为，万物自化                       ║${NC}"
    echo -e "${GREEN}╚══════════════════════════════════════════════════════════════╝${NC}"
    echo ""
}

check_dependencies() {
    local missing=0
    for cmd in docker curl python3; do
        if ! command -v "$cmd" &> /dev/null; then
            error "$cmd 未安装"
            missing=1
        fi
    done

    if ! docker compose version &> /dev/null 2>&1; then
        error "Docker Compose 未安装"
        missing=1
    fi

    if [ $missing -eq 1 ]; then
        exit 1
    fi
}

# 隐藏输入读取密码
read_password() {
    local prompt_msg="${1:-}"
    local var_name="${2:-}"

    while true; do
        prompt "$prompt_msg"
        read -s pw1
        echo ""
        [ -z "$pw1" ] && { warn "密码不能为空"; continue; }
        [ ${#pw1} -lt 8 ] && { warn "密码至少 8 位"; continue; }

        prompt "确认密码: "
        read -s pw2
        echo ""

        [ "$pw1" = "$pw2" ] && { eval "$var_name='$pw1'"; break; }
        warn "两次密码不一致"
    done
}

# 跨平台 sed
sed_inplace() {
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "$@"
    else
        sed -i "$@"
    fi
}

generate_token() {
    python3 -c 'import uuid,secrets; print(f"{uuid.uuid4()}_{secrets.token_hex(8)}")'
}

refresh_admin_tokens() {
    local read_token
    local write_token
    local changed=0

    read_token="$(grep -E '^[[:space:]]*ADMIN_READ_TOKEN=' .env | tail -n 1 | cut -d'=' -f2- || true)"
    write_token="$(grep -E '^[[:space:]]*ADMIN_WRITE_TOKEN=' .env | tail -n 1 | cut -d'=' -f2- || true)"

    if [ -z "${read_token:-}" ]; then
        read_token="$(generate_token)"
        if grep -Eq '^[[:space:]]*#?[[:space:]]*ADMIN_READ_TOKEN=' .env; then
            sed_inplace "s|^[[:space:]]*ADMIN_READ_TOKEN=.*|ADMIN_READ_TOKEN=$read_token|" .env
            sed_inplace "s|^[[:space:]]*#[[:space:]]*ADMIN_READ_TOKEN=.*|ADMIN_READ_TOKEN=$read_token|" .env
        else
            printf '\nADMIN_READ_TOKEN=%s\n' "$read_token" >> .env
        fi
        changed=1
    fi

    if [ -z "${write_token:-}" ]; then
        write_token="$(generate_token)"
        if grep -Eq '^[[:space:]]*#?[[:space:]]*ADMIN_WRITE_TOKEN=' .env; then
            sed_inplace "s|^[[:space:]]*ADMIN_WRITE_TOKEN=.*|ADMIN_WRITE_TOKEN=$write_token|" .env
            sed_inplace "s|^[[:space:]]*#[[:space:]]*ADMIN_WRITE_TOKEN=.*|ADMIN_WRITE_TOKEN=$write_token|" .env
        else
            printf 'ADMIN_WRITE_TOKEN=%s\n' "$write_token" >> .env
        fi
        changed=1
    fi

    if [ "$changed" -eq 1 ]; then
        success "已补齐 .env 管理员凭证"
    else
        info "保留现有 .env 管理员凭证"
    fi
}


# ============================================================================
# 环境配置
# ============================================================================

setup_env() {
    cd "$PROJECT_ROOT"

    if [ -f ".env" ]; then
        refresh_admin_tokens
        return 0
    fi

    info "配置环境变量..."
    cp .env.example .env

    echo ""
    echo -e "${YELLOW}数据库配置${NC}"
    read_password "数据库密码 (至少8位): " DB_PASSWORD

    sed_inplace "s/^DB_PASSWORD=.*/DB_PASSWORD=$DB_PASSWORD/" .env
    sed_inplace "s|postgres://postgres:changeme@|postgres://postgres:$DB_PASSWORD@|" .env

    echo ""
    echo -e "${YELLOW}运行环境${NC}"
    echo "  1) development (debug 日志)"
    echo "  2) production (warn 日志)"
    prompt "选择 [1]: "
    read env_choice

    if [ "${env_choice:-1}" = "2" ]; then
        sed_inplace 's/^RUST_LOG=.*/RUST_LOG=warn/' .env
        sed_inplace 's/^ENVIRONMENT=.*/ENVIRONMENT=production/' .env
    fi

    refresh_admin_tokens
    success "配置完成"
}

check_prod_env() {
    cd "$PROJECT_ROOT"

    local current_pw=$(grep "^DB_PASSWORD=" .env 2>/dev/null | cut -d'=' -f2)
    if [ "$current_pw" = "changeme" ]; then
        warn "生产环境不能使用默认密码！"
        read_password "设置新密码: " DB_PASSWORD
        sed_inplace "s/^DB_PASSWORD=.*/DB_PASSWORD=$DB_PASSWORD/" .env
        sed_inplace "s|postgres://postgres:changeme@|postgres://postgres:$DB_PASSWORD@|" .env
    fi

    sed_inplace 's/^RUST_LOG=.*/RUST_LOG=warn/' .env
    sed_inplace 's/^ENVIRONMENT=.*/ENVIRONMENT=production/' .env
}

ghcr_login() {
    echo ""
    prompt "需要登录 GHCR？(y/N): "
    read need
    if [ "$need" = "y" ] || [ "$need" = "Y" ]; then
        prompt "GitHub 用户名: "
        read user
        prompt "GitHub PAT (read:packages): "
        read -s token
        echo ""
        echo "$token" | docker login ghcr.io -u "$user" --password-stdin || error "登录失败"
        success "登录成功"
    fi
}

# ============================================================================
# 服务管理
# ============================================================================

cmd_start() {
    local mode="${1:-dev}"
    cd "$PROJECT_ROOT"

    check_dependencies

    # 确保 logs 目录存在且权限正确（容器内用户 UID 1000）
    mkdir -p logs
    chmod 755 logs
    # 尝试设置所有者为 1000:1000，如果当前用户不是 root 则忽略错误
    chown 1000:1000 logs 2>/dev/null || true

    if [ "$mode" = "prod" ]; then
        setup_env
        check_prod_env
        ghcr_login
        info "启动服务..."
        docker compose -f docker-compose.prod.yml up -d
    else
        setup_env
        info "启动服务..."
        docker compose up -d
    fi

    info "等待就绪..."
    sleep 5
    for i in {1..30}; do
        curl -s http://localhost:23333/health > /dev/null 2>&1 && break
        [ $i -eq 30 ] && error "启动超时"
        sleep 2
    done

    cmd_status
}

cmd_stop() {
    cd "$PROJECT_ROOT"
    info "停止服务..."
    docker compose down 2>/dev/null || true
    docker compose -f docker-compose.prod.yml down 2>/dev/null || true
    success "已停止"
}

cmd_restart() {
    cmd_stop
    sleep 2
    cmd_start "${1:-}"
}

cmd_status() {
    cd "$PROJECT_ROOT"
    echo ""
    docker compose ps 2>/dev/null || docker compose -f docker-compose.prod.yml ps 2>/dev/null

    echo ""
    info "健康检查:"
    curl -s http://localhost:23333/health | python3 -m json.tool 2>/dev/null || \
        curl -s http://localhost:23333/health || warn "服务未响应"

    local ip=$(curl -s --connect-timeout 2 ifconfig.me 2>/dev/null || echo "localhost")
    echo ""
    echo -e "${GREEN}服务地址:${NC}"
    echo "  HTTP:  http://${ip}:23333"
    echo "  WS:    ws://${ip}:23333/ws?token=YOUR_TOKEN"
    echo ""
    echo -e "${GREEN}管理员访问凭证:${NC}"
    echo "  在.env里面"
    echo ""
}

cmd_logs() {
    cd "$PROJECT_ROOT"
    docker compose logs -f server 2>/dev/null || \
        docker compose -f docker-compose.prod.yml logs -f server
}

cmd_reset() {
    cd "$PROJECT_ROOT"
    warn "将删除所有数据！"
    prompt "确认？(y/N): "
    read confirm
    if [ "$confirm" = "y" ] || [ "$confirm" = "Y" ]; then
        docker compose down -v 2>/dev/null || true
        docker compose -f docker-compose.prod.yml down -v 2>/dev/null || true
        rm -f .env
        success "已清空"
        cmd_start
    fi
}

# ============================================================================
# 构建
# ============================================================================

cmd_build() {
    cd "$PROJECT_ROOT"

    if [ "${1:-}" = "--docker" ]; then
        check_dependencies
        info "构建 Docker 镜像..."
        docker compose build server
    elif [ "${1:-}" = "--release" ]; then
        info "Release 构建..."
        cargo build --workspace --release
    else
        info "Debug 构建..."
        cargo build --workspace
    fi
    success "构建完成"
}

cmd_test() {
    cd "$PROJECT_ROOT"
    info "运行测试..."
    cargo test --workspace
    success "测试通过"
}

# ============================================================================
# 帮助
# ============================================================================

show_help() {
    show_banner
    echo "用法: $0 <命令> [参数]"
    echo ""
    echo -e "${GREEN}服务管理:${NC}"
    echo "  start [--prod]     启动服务 (默认开发环境)"
    echo "  stop               停止服务"
    echo "  restart [--prod]   重启服务"
    echo "  status             查看状态"
    echo "  logs               查看日志"
    echo "  reset              重置所有数据"
    echo ""
    echo -e "${GREEN}构建测试:${NC}"
    echo "  build [--release|--docker]  构建 (默认本地 debug)"
    echo "  test               运行测试"
    echo ""
    echo -e "${GREEN}示例:${NC}"
    echo "  $0 start                    # 开发环境启动"
    echo "  $0 start --prod             # 生产环境启动"
    echo ""
}

# ============================================================================
# 主入口
# ============================================================================

main() {
    case "${1:-help}" in
        start)      cmd_start "${2:-}" ;;
        stop)       cmd_stop ;;
        restart)    cmd_restart "${2:-}" ;;
        status)     show_banner; cmd_status ;;
        logs)       cmd_logs ;;
        reset)      cmd_reset ;;
        build)      cmd_build "${2:-}" ;;
        test)       cmd_test ;;
        help|--help|-h) show_help ;;
        *)
            echo "未知命令: ${1:-}"
            show_help
            exit 1
            ;;
    esac
}

main "$@"
