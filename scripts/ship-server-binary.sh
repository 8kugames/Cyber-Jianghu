#!/usr/bin/env bash
# 跨平台编译 → SCP → 服务器重建 image → 验证 → 还原 Dockerfile
#
# 用途：本地 Mac ARM 编译 linux/amd64 二进制，绕过小内存服务器 cargo OOM。
# 流程：zigbuild → scp → 远端 patch Dockerfile 用 COPY 替代 cargo →
#       docker compose build → up -d → health check → 还原 Dockerfile。
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

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="x86_64-unknown-linux-gnu"
SERVER="${SERVER:?必须设置 SERVER，例如 admin@47.102.120.116}"
REMOTE_PROJECT="${REMOTE_PROJECT:-/home/admin/Cyber-Jianghu}"
COMPOSE_DIR="${COMPOSE_DIR:-$REMOTE_PROJECT/crates/server}"
HEALTH_TIMEOUT="${HEALTH_TIMEOUT:-60}"

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
echo "[构建] zigbuild $TARGET"
RUSTC="$(rustup which rustc)" cargo zigbuild --release --target "$TARGET" \
    --manifest-path "$PROJECT_ROOT/Cargo.toml" -p cyber-jianghu-server
BIN_HASH="$(md5 -q "$BIN" 2>/dev/null || md5sum "$BIN" | cut -d' ' -f1)"
echo "[构建完成] md5=$BIN_HASH"

echo "[上传] scp + 远端 patch + build + up + verify"
scp -q "$BIN" "$SERVER:~/cyber-jianghu-server"
sync_dirty_to_server
ssh -o BatchMode=yes "$SERVER" bash -s -- "$REMOTE_PROJECT" "$COMPOSE_DIR" \
    "$BIN_HASH" "$HEALTH_TIMEOUT" "${SKIP_VERIFY:-}" <<'REMOTE'
set -e
RP="$1"; CD="$2"; MD5="$3"; TIMEOUT="$4"; SKIP="$5"
BACKUP="$CD/Dockerfile.bak"
trap '[ -f "$BACKUP" ] && mv "$BACKUP" "$CD/Dockerfile"' EXIT

cp -f "$CD/Dockerfile" "$BACKUP"
mv ~/cyber-jianghu-server "$RP/server-bin"

python3 - "$CD/Dockerfile" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1])
src = p.read_text()
new = re.sub(
    r"FROM rust:trixie AS builder.*?(?=^FROM )",
    "FROM scratch AS builder\nCOPY server-bin /app/server-bin\n\n",
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

echo "[完成] 部署成功 (md5=${BIN_HASH:-unknown})"