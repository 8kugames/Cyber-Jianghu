#!/usr/bin/env bash
# build-embedding-image.sh - build the embedding service binary in-container, assemble the runtime image
#
# build-agent-image.sh 的 embedding 同构版。The compile runs NATIVELY inside the
# local-rust-trixie:builder container, with the SAME persistent cargo cache
# volumes (cyj-agent-cargo / cyj-agent-target)：agent 的依赖图包含本 crate 全部
# 重依赖（candle/candle-transformers/tokenizers/axum/tokio），共享 target 卷使本
# 构建近乎全量增量（仅编 embedding 自身，弱机上无冷编译 OOM 风险）。
# 注意：build-agent-image.sh --fresh 会连带清掉共享缓存，之后首次本构建重回冷编译。
#
# 模型烘焙进镜像：hf-mirror 下载 + SHA256 校验（幂等，.bin/model 已就绪则跳过下载，
# 已有文件仍校验）。运行时无状态、不挂数据卷，模型随镜像版本演进（避免旧卷遮蔽
# 新镜像内模型——agent 侧已踩过同类坑）。
#
# Runtime recipe: crates/embedding/Dockerfile.runtime（权威结构：crates/embedding/Dockerfile Stage 2）
#
# Usage:
#   ./scripts/deploy/build-embedding-image.sh [--fresh]
#
#   --fresh   wipe the cargo cache volume first (cache pollution triage)

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BUILDER="local-rust-trixie:builder"
BASE="local-debian-slim:runtime"
# 共享 agent 构建的缓存卷（依赖图重合，增量复用）；--fresh 双脚本等效
CARGO_VOLUME="cyj-agent-cargo"
TARGET_VOLUME="cyj-agent-target"
BIN_OUT="$PROJECT_ROOT/.bin/embedding-bin-linux"
MODEL_REPO="BAAI/bge-small-zh-v1.5"
MODEL_HOST_DIR="$PROJECT_ROOT/.bin/model/bge-small-zh-v1.5"
IMAGE="cyber-jianghu-embedding:latest"

# -- base images: auto-bootstrap when missing (network path: docker pull + apt).
# 与 build-agent-image.sh 同一套基镜像，已存在时零开销。
ensure_base_images() {
    local need_rust=0 need_slim=0
    docker image inspect "$BUILDER" > /dev/null 2>&1 || need_rust=1
    docker image inspect "$BASE"   > /dev/null 2>&1 || need_slim=1
    if [ "$need_rust" -eq 0 ] && [ "$need_slim" -eq 0 ]; then
        echo "[base] ready ($BUILDER / $BASE)"
        return 0
    fi
    echo "[base] missing, bootstrapping via docker run + apt (network required)"
    if [ "$need_rust" -eq 1 ]; then
        docker run --name tmp-rust-builder rust:trixie bash -c \
            "apt-get update -qq && apt-get install -y -qq pkg-config libssl-dev > /dev/null && rm -rf /var/lib/apt/lists/*" \
            && docker commit tmp-rust-builder "$BUILDER" && docker rm tmp-rust-builder > /dev/null \
            || { echo "[error] bootstrap failed: $BUILDER"; exit 1; }
    fi
    if [ "$need_slim" -eq 1 ]; then
        docker run --name tmp-deb-slim debian:trixie-slim bash -c \
            "apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates curl libssl3 > /dev/null && rm -rf /var/lib/apt/lists/*" \
            && docker commit tmp-deb-slim "$BASE" && docker rm tmp-deb-slim > /dev/null \
            || { echo "[error] bootstrap failed: $BASE"; exit 1; }
    fi
    echo "[base] bootstrap done"
}
ensure_base_images

case "${1:-}" in
    --fresh)
        docker volume rm "$CARGO_VOLUME" "$TARGET_VOLUME" > /dev/null 2>&1 || true
        echo "[cache] cargo/target volumes wiped (shared with agent build)" ;;
    "") ;;
    *) echo "usage: $0 [--fresh]"; exit 1 ;;
esac

# -- 0. reclaim disposable docker garbage (dangling images + build cache)
# 只清可再生缓存；不碰 named volumes 与有 tag 镜像（同 build-agent-image.sh 约定）
echo "[cache] prune dangling images + build cache"
docker image prune -f 2>/dev/null | tail -n 1 || true
docker builder prune -f 2>/dev/null | tail -n 1 || true
docker system df 2>/dev/null | head -n 5 || true

if [ -n "$(git -C "$PROJECT_ROOT" status --porcelain 2>/dev/null)" ]; then
    echo "[warn] dirty workspace, these files go into the image:"
    git -C "$PROJECT_ROOT" status --short
fi

# -- 1. in-container native build (workspace mount + shared cargo/target cache volumes)
# CARGO_TARGET_DIR=/target：位于 /work 之外的独立卷（同 agent 构建的 EEXIST 规避）
echo "[build] in-container cargo build --release (cache volumes: $CARGO_VOLUME / $TARGET_VOLUME)"
docker run --rm \
    -v "$PROJECT_ROOT:/work" \
    -v "$CARGO_VOLUME:/usr/local/cargo" \
    -v "$TARGET_VOLUME:/target" \
    -e CARGO_TARGET_DIR=/target \
    -w /work \
    "$BUILDER" \
    bash -c '
set -e
CARGO_HOME=${CARGO_HOME:-/usr/local/cargo}
mkdir -p "$CARGO_HOME"
if [ ! -s "$CARGO_HOME/config.toml" ]; then
    printf "[source.crates-io]\nreplace-with = \"aliyun\"\n\n[source.aliyun]\nregistry = \"sparse+https://mirrors.aliyun.com/crates.io-index/\"\n" \
        > "$CARGO_HOME/config.toml"
fi
cargo build --release -p cyber-jianghu-embedding
'
# 从卷中提取二进制到 .bin/（runtime 镜像 COPY 用）
mkdir -p "$PROJECT_ROOT/.bin"
docker run --rm -v "$TARGET_VOLUME:/x:ro" "$BUILDER" \
    cat /x/release/cyber-jianghu-embedding > "$BIN_OUT"
BIN_HASH="$(md5 -q "$BIN_OUT" 2>/dev/null || md5sum "$BIN_OUT" | cut -d' ' -f1)"
echo "[build done] md5=$BIN_HASH size=$(du -h "$BIN_OUT" | cut -f1)"

# -- 1.5 model fetch (幂等)：hf-mirror 下载 + SHA256 校验，烘焙进 runtime 镜像
mkdir -p "$MODEL_HOST_DIR"
sha256_of() { sha256sum "$1" 2>/dev/null | awk '{print $1}' || shasum -a 256 "$1" | awk '{print $1}'; }
for FILE in config.json tokenizer.json model.safetensors; do
    DEST="$MODEL_HOST_DIR/$FILE"
    if [ ! -s "$DEST" ]; then
        echo "[model] fetching $FILE"
        curl -fsSL "https://hf-mirror.com/${MODEL_REPO}/resolve/main/${FILE}" -o "$DEST"
    else
        echo "[model] $FILE present, skip download"
    fi
    EXPECTED="$(curl -fsSL "https://hf-mirror.com/${MODEL_REPO}/resolve/main/${FILE}.sha256" 2>/dev/null | awk '{print $1}' || true)"
    if [ -n "$EXPECTED" ]; then
        ACTUAL="$(sha256_of "$DEST")"
        if [ "$EXPECTED" != "$ACTUAL" ]; then
            echo "[error] SHA256 mismatch for ${FILE}: expected ${EXPECTED}, got ${ACTUAL}"
            rm -f "$DEST"
            exit 1
        fi
        echo "[model] sha256 ok: $FILE"
    else
        echo "[model] sha256 not available for $FILE, skip verification"
    fi
done

# -- 2. assemble runtime image (runtime-only, seconds; recipe: crates/embedding/Dockerfile.runtime)
echo "[assemble] docker build (runtime-only, seconds)"
( cd "$PROJECT_ROOT" && DOCKER_BUILDKIT=0 docker build \
    -f crates/embedding/Dockerfile.runtime \
    --build-arg BASE="$BASE" \
    --build-arg EMBEDDING_BIN=.bin/embedding-bin-linux \
    --build-arg MODEL_DIR=.bin/model/bge-small-zh-v1.5 \
    --build-arg INSTALL_APT=0 \
    -t "$IMAGE" . ) \
    || { echo "[error] image build failed"; exit 1; }
docker tag "$IMAGE" "cyber-jianghu-embedding:$(git -C "$PROJECT_ROOT" rev-parse --short HEAD)"

# -- 3. verify: binary + model baked in ---------------------------------------
echo "[verify] binary + model self-check inside image"
docker run --rm --entrypoint /bin/sh "$IMAGE" -c \
    'test -x /app/embedding-service && test -s /app/data/models/BAAI/bge-small-zh-v1.5/model.safetensors' \
    || { echo "[fail] image verification failed"; exit 1; }
echo "[pass] /app/embedding-service + model baked in"

echo ""
echo "[done] $IMAGE ready (HEAD $(git -C "$PROJECT_ROOT" rev-parse --short HEAD))"
echo "  next: docker compose -f /home/admin/cj-review-agents/docker-compose.yml up -d embedding"
