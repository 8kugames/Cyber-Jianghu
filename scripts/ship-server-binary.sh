#!/usr/bin/env bash
# 跨平台编译 → SCP → 服务器重建 image → 验证 → 还原 Dockerfile
#
# 用途：本地 Mac ARM 编译 linux/amd64 二进制，绕过小内存服务器 cargo OOM。
# 流程：zigbuild → scp → 远端 patch Dockerfile 用 COPY 替代 cargo →
#       docker compose build → up -d → health check → 还原 Dockerfile → 输出 Admin 凭证。
#
# 用法：
#   SERVER=user@host ./scripts/ship-server-binary.sh
#
# 环境变量：
#   SERVER            必填，形如 admin@47.102.120.116 或 ssh config alias
#   REMOTE_PROJECT    远端项目目录，默认 /home/admin/Cyber-Jianghu
#   COMPOSE_DIR       远端 docker-compose 目录，默认 $REMOTE_PROJECT/crates/server
#   HEALTH_TIMEOUT    健康检查超时秒数，默认 60
#   SKIP_VERIFY       非空则跳过 health 校验
#   MIN_FREE_MB       远端磁盘可用空间警告阈值 MB，默认 1024；清理缓存后仍低于此值则告警

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="x86_64-unknown-linux-gnu"
SERVER="${SERVER:?必须设置 SERVER，例如 admin@47.102.120.116}"
REMOTE_PROJECT="${REMOTE_PROJECT:-/home/admin/Cyber-Jianghu}"
COMPOSE_DIR="${COMPOSE_DIR:-$REMOTE_PROJECT/crates/server}"
HEALTH_TIMEOUT="${HEALTH_TIMEOUT:-60}"
MIN_FREE_MB="${MIN_FREE_MB:-1024}"

command -v cargo-zigbuild >/dev/null || { echo "[错误] 缺 cargo-zigbuild"; exit 1; }
command -v zig >/dev/null || { echo "[错误] 缺 zig"; exit 1; }
rustup which cargo >/dev/null || { echo "[错误] rustup 找不到 cargo"; exit 1; }
rustup target list --installed | grep -qx "$TARGET" \
    || { echo "[错误] 缺 rust target $TARGET"; exit 1; }

if [ -n "$(git -C "$PROJECT_ROOT" status --porcelain 2>/dev/null)" ]; then
    echo "[警告] 工作区有未提交改动,以下文件会被打包进 binary:"
    git -C "$PROJECT_ROOT" status --short
fi

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

echo "[构建] zigbuild $TARGET"
RUSTC="$(rustup which rustc)" cargo zigbuild --release --target "$TARGET" \
    --manifest-path "$PROJECT_ROOT/Cargo.toml" -p cyber-jianghu-server
BIN_HASH="$(md5 -q "$BIN" 2>/dev/null || md5sum "$BIN" | cut -d' ' -f1)"
echo "[构建完成] md5=$BIN_HASH"

echo "[上传] scp + 远端 patch + build + up + verify"
scp -q "$BIN" "$SERVER:~/cyber-jianghu-server"
sync_dirty_to_server
ssh -o BatchMode=yes "$SERVER" bash -s -- "$REMOTE_PROJECT" "$COMPOSE_DIR" \
    "$BIN_HASH" "$HEALTH_TIMEOUT" "${SKIP_VERIFY:-}" "$MIN_FREE_MB" <<'REMOTE'
set -e
RP="$1"; CD="$2"; MD5="$3"; TIMEOUT="$4"; SKIP="$5"; MINFREE="$6"
BACKUP="$CD/Dockerfile.bak"
trap '[ -f "$BACKUP" ] && mv "$BACKUP" "$CD/Dockerfile"' EXIT

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

cp -f "$CD/Dockerfile" "$BACKUP"
mkdir -p "$RP/.bin"
mv ~/cyber-jianghu-server "$RP/.bin/server-bin"

python3 - "$CD/Dockerfile" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1])
src = p.read_text()
new = re.sub(
    r"FROM rust:trixie AS builder.*?(?=^FROM )",
    "FROM scratch AS builder\nCOPY .bin/server-bin /app/server-bin\n\n",
    src, flags=re.DOTALL | re.MULTILINE)
new = new.replace("    cp /app/target/release/cyber-jianghu-server /app/server-bin && \\\n", "")
p.write_text(new)
PY

cd "$CD"
docker compose -f docker-compose.prod.yml build server
docker compose -f docker-compose.prod.yml up -d server

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
