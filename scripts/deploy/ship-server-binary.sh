#!/usr/bin/env bash
# 跨平台编译 → SCP → 服务器打包 runtime 镜像 → 验证 → 输出 Admin 凭证
#
# 用途：本地 Mac ARM 编译 linux/amd64 二进制，绕过小内存服务器 cargo OOM。
# 流程：build-linux-binary.sh（工具链预检 + zigbuild + md5 + 本地留存）→ scp →
#       远端 Dockerfile.runtime COPY 打包（不再 patch 主 Dockerfile）→
#       docker compose up -d → health check → 输出 Admin 凭证。
#
# 用法：
#   SERVER=user@host REMOTE_PROJECT=/path/to/Cyber-Jianghu \
#       ./scripts/deploy/ship-server-binary.sh
#
# 环境变量：
#   SERVER            必填，形如 user@host 或 ssh config alias（禁止写入真实线上地址）
#   REMOTE_PROJECT    必填，远端项目目录（禁止内置默认值）
#   COMPOSE_DIR       远端 docker-compose 目录，默认 $REMOTE_PROJECT/crates/server
#   SERVER_IMAGE      server 镜像 tag，默认 cyber-jianghu-server:binary-runtime，
#                     须与 crates/server/docker-compose.prod.yml 的 server.image 一致
#   HEALTH_TIMEOUT    健康检查超时秒数，默认 60
#   SKIP_VERIFY       非空则跳过 health 校验
#   MIN_FREE_MB       远端磁盘可用空间警告阈值 MB，默认 1024；清理缓存后仍低于此值则告警

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TARGET="x86_64-unknown-linux-gnu"
SERVER="${SERVER:?必须设置 SERVER，形如 user@host}"
REMOTE_PROJECT="${REMOTE_PROJECT:?必须设置 REMOTE_PROJECT，远端项目目录}"
COMPOSE_DIR="${COMPOSE_DIR:-$REMOTE_PROJECT/crates/server}"
HEALTH_TIMEOUT="${HEALTH_TIMEOUT:-60}"
MIN_FREE_MB="${MIN_FREE_MB:-1024}"
# 须与 crates/server/docker-compose.prod.yml 的 server.image 一致（tag 是
# compose up -d 判断是否重建容器的依据）
SERVER_IMAGE="${SERVER_IMAGE:-cyber-jianghu-server:binary-runtime}"

# 工具链预检 / dirty 警告 / zigbuild / md5 / .bin 本地留存统一在
# build-linux-binary.sh（与 .test-agents/push_server.sh 的 agent 构建共享），
# 见下方 [构建] 步骤

# 同步本地 dirty 文件到服务端 build context（Dockerfile COPY 走服务端路径）
sync_dirty_to_server() {
    [ -z "$(git -C "$PROJECT_ROOT" status --porcelain 2>/dev/null)" ] && return 0
    echo "[同步] 本地 dirty 文件 → 服务端 build context"
    while IFS= read -r line; do
        xy="${line:0:2}"
        f="${line:3}"
        case "$xy" in
            *M*|*A*|*??*)  # modified / added / untracked
                [ -f "$PROJECT_ROOT/$f" ] || continue
                # 服务端父目录不存在则跳过（避免污染服务端）
                ssh -o BatchMode=yes "$SERVER" test -d "$(dirname "$REMOTE_PROJECT/$f")" \
                    || { echo "[ skip ] $f (服务端无此目录)"; continue; }
                scp -q "$PROJECT_ROOT/$f" "$SERVER:$REMOTE_PROJECT/$f" \
                    && echo "[ sync ] $f"
                ;;
            *D*)  # deleted
                ssh -o BatchMode=yes "$SERVER" "rm -f '$REMOTE_PROJECT/$f'" \
                    && echo "[ rm ] $f"
                ;;
        esac
    done < <(git -C "$PROJECT_ROOT" status --porcelain)
}

BIN="$PROJECT_ROOT/target/$TARGET/release/cyber-jianghu-server"

# 远端必须能解析 rsync（rsync over ssh 要求两端都有二进制，否则远端 fish/bash
# 把 "rsync --server" 当未知命令，pipe 早断报 "unexpected end of file"）。
# 默认仅自检报错；AUTO_INSTALL_DEPS=1 时探测包管理器并自动 sudo 安装。
if ! ssh -o BatchMode=yes "$SERVER" 'command -v rsync >/dev/null'; then
    if [ "${AUTO_INSTALL_DEPS:-0}" = "1" ]; then
        echo "[自检] 远端缺 rsync，AUTO_INSTALL_DEPS=1 启用自动安装"
        ssh -o BatchMode=yes "$SERVER" '
            if command -v dnf >/dev/null; then PM=dnf
            elif command -v yum >/dev/null; then PM=yum
            elif command -v apt-get >/dev/null; then PM=apt-get
            else echo "[错误] 未识别包管理器，请手动安装 rsync" >&2; exit 1; fi
            sudo "$PM" install -y rsync
            command -v rsync >/dev/null || { echo "[错误] 安装后仍未找到 rsync" >&2; exit 1; }
        ' || { echo "[错误] 自动安装 rsync 失败（检查 sudo NOPASSWD / 源可达性）"; exit 1; }
    else
        echo "[错误] 远端缺 rsync，请执行: ssh $SERVER 'sudo yum install -y rsync'，或重跑时带 AUTO_INSTALL_DEPS=1"
        exit 1
    fi
fi

echo "[同步] config/ → 服务端（防二进制新/配置旧错配）"
rsync -az --delete "$PROJECT_ROOT/crates/server/config/" "$SERVER:$REMOTE_PROJECT/crates/server/config/"     && echo "[ sync ] config/ 全量同步"

# static/ 与 config/ 同属构建上下文输入（Dockerfile: COPY crates/server/static /app/static）。
# sync_dirty_to_server 只覆盖 git 未提交改动，已提交的静态修复（如管理面板 JS）不会进入
# 远端上下文，镜像内 /app/static 会停留旧副本——表现为服务端版本已更新、面板仍跑旧 JS。
echo "[同步] static/ → 服务端（防二进制新/静态旧错配）"
rsync -az --delete "$PROJECT_ROOT/crates/server/static/" "$SERVER:$REMOTE_PROJECT/crates/server/static/"   && echo "[ sync ] static/ 全量同步"

echo "[同步] Dockerfile.runtime → 服务端（显式传送，不依赖远端 git 状态）"
scp -q "$PROJECT_ROOT/crates/server/Dockerfile.runtime" \
    "$SERVER:$REMOTE_PROJECT/crates/server/Dockerfile.runtime" \
    && echo "[ sync ] Dockerfile.runtime"

# migrations 与 entrypoint 同为 Dockerfile.runtime 的 COPY 输入，与 config/static
# 同级全量同步；compose 文件是部署契约（server.image tag 配对），远端 git 陈旧
# 会导致 compose 解析旧镜像名 → up -d 静默空操作（已实证的失败模式）
echo "[同步] migrations/ + entrypoint + compose → 服务端"
rsync -az --delete "$PROJECT_ROOT/crates/server/migrations/" "$SERVER:$REMOTE_PROJECT/crates/server/migrations/" && echo "[ sync ] migrations/ 全量同步"
scp -q "$PROJECT_ROOT/crates/server/docker-entrypoint.sh" \
    "$SERVER:$REMOTE_PROJECT/crates/server/docker-entrypoint.sh" \
    && echo "[ sync ] docker-entrypoint.sh"
scp -q "$PROJECT_ROOT/crates/server/docker-compose.prod.yml" \
    "$SERVER:$COMPOSE_DIR/docker-compose.prod.yml" \
    && echo "[ sync ] docker-compose.prod.yml"

echo "[构建] 交叉编译（委托 build-linux-binary.sh）"
# 经 bash 调用：不依赖文件可执行位，避免部署中途因 +x 缺失报 Permission denied
BIN_HASH="$(bash "$PROJECT_ROOT/scripts/deploy/build-linux-binary.sh" -p cyber-jianghu-server)"

echo "[上传] scp + 远端打包 + up + verify"
scp -q "$BIN" "$SERVER:~/cyber-jianghu-server"
sync_dirty_to_server
# 单引号逐个包裹：ssh 拼接会丢参数边界，空 SKIP_VERIFY 位被吞后参数
# 前移占位，导致默认路径静默跳过 health 验证（已实证的潜伏 bug）
ssh -o BatchMode=yes "$SERVER" \
    "bash -s -- '$REMOTE_PROJECT' '$COMPOSE_DIR' '$BIN_HASH' '$HEALTH_TIMEOUT' '${SKIP_VERIFY:-}' '$MIN_FREE_MB' '$SERVER_IMAGE'" <<'REMOTE'
set -e
RP="$1"; CD="$2"; MD5="$3"; TIMEOUT="$4"; SKIP="$5"; MINFREE="$6"; IMG="$7"

# 磁盘水位检查 + 可再生缓存清理（防 VM 磁盘满导致 build 失败）：
# 只清 dangling images 与 build cache（旧版 server 镜像 up -d 后变 dangling，主体垃圾源），
# 不碰 named volumes（postgres 数据卷）与在用镜像，均可再生。
free_mb() { df -Pm / | awk 'NR==2{print $4}'; }
FREE_BEFORE="$(free_mb)"
docker image prune -f >/dev/null 2>&1 || true
docker builder prune -f 2>&1 | tail -n 1 || true
FREE_AFTER="$(free_mb)"
echo "[磁盘] 清理前 ${FREE_BEFORE}MB → 清理后可用 ${FREE_AFTER}MB"
if [ "${FREE_AFTER:-0}" -lt "$MINFREE" ]; then
    echo "[警告] 磁盘可用 ${FREE_AFTER}MB 低于阈值 ${MINFREE}MB，build 可能失败（可调 MIN_FREE_MB 或手动清理）"
fi

mkdir -p "$RP/.bin"
mv ~/cyber-jianghu-server "$RP/.bin/server-bin"

# 二进制注入打包：配方单一来源 crates/server/Dockerfile.runtime（本地已显式
# 同步到远端），只做 COPY 打包，秒级完成，不再 patch 主 Dockerfile。镜像 tag
# 与 docker-compose.prod.yml 的 server.image 一致，up -d 依据 tag 指向的新
# 镜像 ID 自动重建容器。
docker build -q -f "$RP/crates/server/Dockerfile.runtime" \
    --build-arg SERVER_BIN=.bin/server-bin \
    -t "$IMG" "$RP" >/dev/null
echo "[镜像] $IMG 已重建 (md5=$MD5)"

cd "$CD"
docker compose -f docker-compose.prod.yml up -d server

# 重建核验：容器实际镜像 ID 必须等于刚打包的 tag，否则 compose 因配方漂移
# 解析到旧镜像名会静默 no-op（已实证：远端 compose 旧版无 image: 键时，
# 新镜像空挂、旧容器继续运行、health 假阳性通过）
fail=0
for cid in $(docker compose -f docker-compose.prod.yml ps -q server); do
    run_img="$(docker inspect -f '{{.Image}}' "$cid")"
    if [ "$run_img" != "$(docker image inspect -f '{{.Id}}' "$IMG")" ]; then
        echo "[失败] 容器未运行新镜像（running=${run_img:0:19}），检查 $CD/docker-compose.prod.yml 的 server.image 是否为 $IMG" >&2
        fail=1
    else
        echo "[核验] 容器已运行新镜像 $IMG (${run_img:0:19})"
    fi
done
[ "$fail" = "0" ] || exit 1

if [ -n "$SKIP" ]; then exit 0; fi
echo "[验证] 等 health 通过 (timeout ${TIMEOUT}s)"
pass=0
for i in $(seq 1 "$TIMEOUT"); do
    body="$(curl -sf -m 3 http://localhost:23333/health 2>/dev/null || true)"
    if echo "$body" | grep -q '"status":"ok"'; then
        echo "[通过] $body"
        pass=1
        break
    fi
    sleep 1
done
[ "$pass" = "1" ] || { echo "[失败] health 未通过"; exit 1; }
REMOTE

# 读取并展示服务端 Admin 访问凭证（token 文件由 server 启动时写入）
print_admin_credentials() {
    local creds read_key write_key host_part
    host_part="${SERVER##*@}"
    echo "[凭证] 读取 Admin token (logs/cyber_jianghu_admin.tmp)"
    # 等待窗口覆盖 SKIP_VERIFY 场景：容器启动后才会刷新 token 文件
    if ! creds="$(ssh -o BatchMode=yes "$SERVER" bash -s -- "$COMPOSE_DIR" <<'TOKENS'
set -e
F="$1/logs/cyber_jianghu_admin.tmp"
for i in $(seq 1 15); do
    [ -f "$F" ] && grep -q 'Read Token' "$F" 2>/dev/null && break
    sleep 1
done
if [ ! -f "$F" ] || ! grep -q 'Read Token' "$F" 2>/dev/null; then
    echo "token 文件未就绪: $F (服务完全启动后重跑或上服务器查看)" >&2
    exit 1
fi
awk '/Read Token/{getline; gsub(/^[ \t]+/,""); print "READ_TOKEN=" $0; exit}' "$F"
awk '/Write Token/{getline; gsub(/^[ \t]+/,""); print "WRITE_TOKEN=" $0; exit}' "$F"
TOKENS
)"; then
        echo "[警告] token 读取失败,不影响部署结果;可稍后查看服务器 $COMPOSE_DIR/logs/cyber_jianghu_admin.tmp"
        return 0
    fi
    read_key="$(printf '%s\n' "$creds" | sed -n 's/^READ_TOKEN=//p' | head -1)"
    write_key="$(printf '%s\n' "$creds" | sed -n 's/^WRITE_TOKEN=//p' | head -1)"

    echo
    echo "============================================================"
    echo " Admin 访问凭证"
    echo "============================================================"
    echo " 只读 Key (read) : ${read_key:-<解析失败>}"
    echo " 读写 Key (write): ${write_key:-<解析失败>}"
    if [ -n "$read_key" ]; then
        echo
        echo " 只读直达链接(#token= 不进服务器日志):"
        echo " http://${host_part}:23333/admin/index.html#token=${read_key}"
    fi
    echo "============================================================"
}

echo "[完成] 部署成功 (md5=${BIN_HASH:-unknown})"
print_admin_credentials
