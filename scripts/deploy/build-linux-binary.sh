#!/usr/bin/env bash
# build-linux-binary.sh - 交叉编译 linux 二进制 + md5 + .bin 本地留存（共享助手）
#
# 单一来源收敛 zigbuild 交叉编译的前置检查与产物处理：
#   - scripts/deploy/ship-server-binary.sh   (cyber-jianghu-server)
#   - .test-agents/push_server.sh            (cyber-jianghu-agent)
#
# stdout 仅输出 md5（供调用方命令替换捕获），全部进度日志走 stderr
# （命令替换下 stderr 仍实时可见，且不污染 md5 捕获）。
#
# 用法：
#   MD5="$(./scripts/deploy/build-linux-binary.sh -p cyber-jianghu-server)"
#   MD5="$(./scripts/deploy/build-linux-binary.sh -p cyber-jianghu-agent)"
#
# 产物：target/x86_64-unknown-linux-gnu/release/<pkg>，
#       副本 .bin/<short>-bin（cyber-jianghu-server → server-bin，
#       cyber-jianghu-agent → agent-bin），与远端 .bin/ 命名约定一致，
#       便于核对 md5 / 回滚。

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TARGET="x86_64-unknown-linux-gnu"

case "${1:-}" in
    -p) PKG="${2:?-p 需要包名，如 cyber-jianghu-server}" ;;
    *)  echo "usage: $0 -p <package>（如 cyber-jianghu-server / cyber-jianghu-agent）" >&2; exit 1 ;;
esac
BIN_NAME="${PKG#cyber-jianghu-}-bin"

command -v cargo-zigbuild >/dev/null || { echo "[错误] 缺 cargo-zigbuild" >&2; exit 1; }
command -v zig >/dev/null || { echo "[错误] 缺 zig" >&2; exit 1; }
rustup which cargo >/dev/null || { echo "[错误] rustup 找不到 cargo" >&2; exit 1; }
rustup target list --installed | grep -qx "$TARGET" \
    || { echo "[错误] 缺 rust target $TARGET" >&2; exit 1; }

if [ -n "$(git -C "$PROJECT_ROOT" status --porcelain 2>/dev/null)" ]; then
    echo "[警告] 工作区有未提交改动,以下文件会被打包进 binary:" >&2
    git -C "$PROJECT_ROOT" status --short >&2
fi

echo "[构建] zigbuild $TARGET $PKG" >&2
BIN="$PROJECT_ROOT/target/$TARGET/release/$PKG"
RUSTC="$(rustup which rustc)" cargo zigbuild --release --target "$TARGET" \
    --manifest-path "$PROJECT_ROOT/Cargo.toml" -p "$PKG" >&2

mkdir -p "$PROJECT_ROOT/.bin" && cp "$BIN" "$PROJECT_ROOT/.bin/$BIN_NAME" \
    || echo "[警告] 本地备份失败(不阻塞)" >&2

MD5="$(md5 -q "$BIN" 2>/dev/null || md5sum "$BIN" | cut -d' ' -f1)"
echo "[构建完成] md5=$MD5 (.bin/$BIN_NAME)" >&2
echo "$MD5"
