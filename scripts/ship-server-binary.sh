#!/usr/bin/env bash
# 跨平台编译 → SCP → 服务器重建 image → 验证 → 还原 Dockerfile
#
# 用途：本地 Mac ARM 编译 linux/amd64 二进制，绕过小内存服务器 cargo OOM。
# 流程：zigbuild → scp → ssh 内 patch Dockerfile 用 COPY 替代 cargo →
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
BIN_PATH="$PROJECT_ROOT/target/$TARGET/release/cyber-jianghu-server"

if [ -z "${SERVER:-}" ]; then
    echo "[错误] 必须设置 SERVER，例如 SERVER=admin@47.102.120.116" >&2
    exit 1
fi
REMOTE_PROJECT="${REMOTE_PROJECT:-/home/admin/Cyber-Jianghu}"
COMPOSE_DIR="${COMPOSE_DIR:-$REMOTE_PROJECT/crates/server}"
HEALTH_TIMEOUT="${HEALTH_TIMEOUT:-60}"

# ---------------------------------------------------------------------------
# 前置检查
# ---------------------------------------------------------------------------
command -v cargo-zigbuild >/dev/null 2>&1 || {
    echo "[错误] 缺少 cargo-zigbuild。安装: cargo install cargo-zigbuild" >&2; exit 1; }
command -v zig >/dev/null 2>&1 || {
    echo "[错误] 缺少 zig。安装: brew install zig" >&2; exit 1; }

# 远端 PATH 中 cargo 是 homebrew 版时找不到 rustup 装的目标，显式走 rustup toolchain。
RUSTUP_CARGO="$(rustup which cargo 2>/dev/null || true)"
if [ -z "$RUSTUP_CARGO" ]; then
    echo "[错误] rustup 未识别当前 toolchain。先 rustup default stable" >&2; exit 1
fi
RUSTC_BIN="$(rustup which rustc)"

rustup target list --installed 2>/dev/null | grep -qx "$TARGET" || {
    echo "[错误] 缺少 rust target $TARGET。安装: rustup target add $TARGET" >&2; exit 1; }

echo "[构建] zigbuild $TARGET (本地编译)"
RUSTC="$RUSTC_BIN" cargo zigbuild --release --target "$TARGET" \
    --manifest-path "$PROJECT_ROOT/Cargo.toml" -p cyber-jianghu-server

[ -f "$BIN_PATH" ] || { echo "[错误] 编译后未找到 $BIN_PATH" >&2; exit 1; }
BIN_MD5="$(md5 -q "$BIN_PATH" 2>/dev/null || md5sum "$BIN_PATH" | cut -d' ' -f1)"
echo "[构建完成] $BIN_PATH  md5=$BIN_MD5"

# ---------------------------------------------------------------------------
# SCP 上传 + 远端准备
# ---------------------------------------------------------------------------
echo "[上传] scp → $SERVER"
scp -q "$BIN_PATH" "$SERVER:~/cyber-jianghu-server"

echo "[远端] 备份 Dockerfile + 放置 binary + 改写 builder 阶段"
ssh -o BatchMode=yes "$SERVER" bash -s -- "$REMOTE_PROJECT" <<'REMOTE'
set -e
RP="$1"
cp -f "$RP/crates/server/Dockerfile" "$RP/crates/server/Dockerfile.bak.$(date +%s)"
mv ~/cyber-jianghu-server "$RP/server-bin"

RP="$1"
cd "$RP/crates/server"
python3 - "$PWD/Dockerfile" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1])
src = p.read_text()
# 把 builder 阶段（FROM rust:trixie AS builder 到下一个 FROM 之前）替换成 FROM scratch + COPY
new = re.sub(
    r"FROM rust:trixie AS builder.*?(?=^FROM )",
    "FROM scratch AS builder\nCOPY server-bin /app/server-bin\n\n",
    src,
    flags=re.DOTALL | re.MULTILINE,
)
# builder 阶段里 cp /app/target/release/... 行变成多余（已直接 COPY），删掉
new = new.replace("    cp /app/target/release/cyber-jianghu-server /app/server-bin && \\\n", "")
p.write_text(new)
print("[Dockerfile patched]")
PY
REMOTE

# ---------------------------------------------------------------------------
# 远端 build image + 重启 server
# ---------------------------------------------------------------------------
echo "[远端] docker compose build + up -d server"
ssh -o BatchMode=yes "$SERVER" bash -s -- "$COMPOSE_DIR" <<'REMOTE'
set -e
cd "$1"
docker compose -f docker-compose.prod.yml build server
docker compose -f docker-compose.prod.yml up -d server
REMOTE

# ---------------------------------------------------------------------------
# 健康检查
# ---------------------------------------------------------------------------
if [ -n "${SKIP_VERIFY:-}" ]; then
    echo "[跳过] SKIP_VERIFY 已设置"
else
    echo "[验证] 等 health 通过（timeout ${HEALTH_TIMEOUT}s）"
    ok=0
    for i in $(seq 1 "$HEALTH_TIMEOUT"); do
        body="$(curl -sf -m 3 http://"$SERVER":23333/health 2>/dev/null || true)"
        if echo "$body" | grep -q '"status":"ok"'; then
            echo "[通过] $body"
            ok=1
            break
        fi
        sleep 1
    done
    [ "$ok" = "1" ] || { echo "[失败] health 未通过" >&2; exit 1; }
fi

# ---------------------------------------------------------------------------
# 还原 Dockerfile（关键！否则下次 install.sh rebuild 找不到 cargo 上下文）
# ---------------------------------------------------------------------------
echo "[收尾] 还原 Dockerfile"
ssh -o BatchMode=yes "$SERVER" bash -s -- "$REMOTE_PROJECT" <<'REMOTE'
set -e
RP="$1"
bak="$(ls -t "$RP/crates/server/Dockerfile.bak."* 2>/dev/null | head -1 || true)"
if [ -z "$bak" ]; then
    echo "[错误] 找不到 Dockerfile 备份" >&2; exit 1
fi
mv "$bak" "$RP/crates/server/Dockerfile"
echo "[Dockerfile 还原自 $bak]"
REMOTE

echo "[完成] 部署成功"