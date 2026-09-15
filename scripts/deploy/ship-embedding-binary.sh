#!/usr/bin/env bash
# 交叉编译 → SCP → 服务器组装 runtime 镜像 → 验证（embedding 服务 x86_64 运维管线）
#
# ship-server-binary.sh 的 embedding 同构版。用途：本地 Mac ARM 编译 linux/amd64
# 二进制，绕过小内存服务器 cargo OOM（服务器只做 COPY 组装，秒级完成）。
# 流程：build-linux-binary.sh（zigbuild 交叉编译 + md5 + 本地留存）→
#       scp 二进制与 Dockerfile.runtime → 模型本地直传（.bin/model/，幂等）→
#       docker build 组装 → compose up -d → health + embed 冒烟验证。
#
# 模型为架构无关静态文件，默认从本地 .bin/model/ 直传（避免服务器重新下载
# ~100MB）；本地副本缺失时回退为远端 hf-mirror 下载（+SHA256 校验）。两种来源
# 均在远端校验 sha256（上游校验文件不可达时告警放行）。
#
# 本地模型副本制备（从开发栈 docker 卷提取，一次性）：
#   mkdir -p .bin/model && docker run --rm \
#     -v cyber-jianghu-embedding-data:/x:ro -v "$PWD/.bin/model:/out" \
#     alpine cp -r /x/models/BAAI/bge-small-zh-v1.5 /out/
#
# 用法：
#   SERVER=user@host REMOTE_PROJECT=/path/to/Cyber-Jianghu \
#     COMPOSE_DIR=/path/to/cj-review-agents \
#     ./scripts/deploy/ship-embedding-binary.sh
#
# 环境变量：
#   SERVER            必填，形如 user@host 或 ssh config alias（禁止写入真实线上地址）
#   REMOTE_PROJECT    必填，远端项目仓库目录（build context 与模型缓存根）
#   COMPOSE_DIR       必填，远端 docker-compose 目录（含 embedding 服务的 compose 文件）
#   EMBEDDING_IMAGE   镜像 tag，默认 cyber-jianghu-embedding:latest，
#                     须与 compose 文件 embedding.image 一致
#   MODEL_REPO        HuggingFace 模型仓，默认 BAAI/bge-small-zh-v1.5
#   MODEL_LOCAL       本地模型目录（含 config.json/tokenizer.json/model.safetensors），
#                     默认 $PROJECT_ROOT/.bin/model/bge-small-zh-v1.5；置空强制远端下载
#   HEALTH_TIMEOUT    健康检查超时秒数，默认 60
#   SKIP_VERIFY       非空则跳过 health/embed 冒烟校验

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SERVER="${SERVER:?必须设置 SERVER，形如 user@host}"
REMOTE_PROJECT="${REMOTE_PROJECT:?必须设置 REMOTE_PROJECT，远端项目目录}"
COMPOSE_DIR="${COMPOSE_DIR:?必须设置 COMPOSE_DIR，远端 embedding compose 目录}"
EMBEDDING_IMAGE="${EMBEDDING_IMAGE:-cyber-jianghu-embedding:latest}"
MODEL_REPO="${MODEL_REPO:-BAAI/bge-small-zh-v1.5}"
# 无冒号形式：允许 MODEL_LOCAL="" 显式禁用本地上传（走远端下载）
MODEL_LOCAL="${MODEL_LOCAL-$PROJECT_ROOT/.bin/model/bge-small-zh-v1.5}"
HEALTH_TIMEOUT="${HEALTH_TIMEOUT:-60}"

BIN="$PROJECT_ROOT/target/x86_64-unknown-linux-gnu/release/cyber-jianghu-embedding"

echo "[同步] Dockerfile.runtime → 服务端（显式传送，不依赖远端 git 状态）"
ssh -o BatchMode=yes "$SERVER" "test -d '$REMOTE_PROJECT/crates/embedding'" \
    || { echo "[错误] 远端缺 $REMOTE_PROJECT/crates/embedding 目录"; exit 1; }
scp -q "$PROJECT_ROOT/crates/embedding/Dockerfile.runtime" \
    "$SERVER:$REMOTE_PROJECT/crates/embedding/Dockerfile.runtime" \
    && echo "[ sync ] Dockerfile.runtime"

echo "[构建] 交叉编译（委托 build-linux-binary.sh）"
BIN_HASH="$(bash "$PROJECT_ROOT/scripts/deploy/build-linux-binary.sh" -p cyber-jianghu-embedding)"

echo "[上传] scp 二进制 + 远端组装 + up + verify"
scp -q "$BIN" "$SERVER:~/cyber-jianghu-embedding"

# 模型本地直传（幂等覆盖，远端组装时复用；本地副本缺失时由远端下载兜底）
MODEL_FILES_OK=1
for f in config.json tokenizer.json model.safetensors; do
    [ -s "$MODEL_LOCAL/$f" ] || MODEL_FILES_OK=0
done
if [ "$MODEL_FILES_OK" = "1" ]; then
    echo "[上传] 模型本地直传: $MODEL_LOCAL → $REMOTE_PROJECT/.bin/model/bge-small-zh-v1.5"
    ssh -o BatchMode=yes "$SERVER" "mkdir -p '$REMOTE_PROJECT/.bin/model/bge-small-zh-v1.5'"
    scp -q "$MODEL_LOCAL/config.json" "$MODEL_LOCAL/tokenizer.json" \
        "$MODEL_LOCAL/model.safetensors" \
        "$SERVER:$REMOTE_PROJECT/.bin/model/bge-small-zh-v1.5/"
else
    echo "[模型] 本地副本缺失（$MODEL_LOCAL），远端将从 hf-mirror 下载"
fi
# 单引号逐个包裹：ssh 拼接会丢参数边界（见 ship-server-binary.sh 既有教训）
ssh -o BatchMode=yes "$SERVER" \
    "bash -s -- '$REMOTE_PROJECT' '$COMPOSE_DIR' '$BIN_HASH' '$HEALTH_TIMEOUT' '${SKIP_VERIFY:-}' '$EMBEDDING_IMAGE' '$MODEL_REPO'" <<'REMOTE'
set -e
RP="$1"; CD="$2"; MD5="$3"; TIMEOUT="$4"; SKIP="$5"; IMG="$6"; MREPO="$7"

# 磁盘水位检查 + 可再生缓存清理（同 ship-server-binary.sh 约定：
# 只清 dangling 与 build cache，不碰 named volumes 与在用镜像）
free_mb() { df -Pm / | awk 'NR==2{print $4}'; }
FREE_BEFORE="$(free_mb)"
docker image prune -f >/dev/null 2>&1 || true
docker builder prune -f 2>&1 | tail -n 1 || true
FREE_AFTER="$(free_mb)"
echo "[磁盘] 清理前 ${FREE_BEFORE}MB → 清理后可用 ${FREE_AFTER}MB"

mkdir -p "$RP/.bin"
mv ~/cyber-jianghu-embedding "$RP/.bin/embedding-bin"

# 模型下载（幂等）：hf-mirror + SHA256 校验，缓存于 .bin/model/ 供组装复用
MODEL_DIR="$RP/.bin/model/bge-small-zh-v1.5"
mkdir -p "$MODEL_DIR"
for FILE in config.json tokenizer.json model.safetensors; do
    DEST="$MODEL_DIR/$FILE"
    if [ ! -s "$DEST" ]; then
        echo "[模型] 下载 $FILE"
        curl -fsSL "https://hf-mirror.com/${MREPO}/resolve/main/${FILE}" -o "$DEST"
    else
        echo "[模型] $FILE 已缓存，跳过下载"
    fi
    EXPECTED="$(curl -fsSL "https://hf-mirror.com/${MREPO}/resolve/main/${FILE}.sha256" 2>/dev/null | awk '{print $1}' || true)"
    if [ -n "$EXPECTED" ]; then
        ACTUAL="$(sha256sum "$DEST" | awk '{print $1}')"
        if [ "$EXPECTED" != "$ACTUAL" ]; then
            echo "[错误] $FILE SHA256 不匹配: expect ${EXPECTED} got ${ACTUAL}" >&2
            rm -f "$DEST"; exit 1
        fi
        echo "[模型] sha256 ok: $FILE"
    else
        echo "[模型] $FILE 无上游 sha256，跳过校验"
    fi
done

# 二进制注入组装：配方 crates/embedding/Dockerfile.runtime（本脚本已显式同步到远端），
# 仅 COPY 打包，秒级完成。公网基镜像 debian:trixie-slim，apt 层由 Dockerfile 按
# INSTALL_APT=1 默认安装（服务器内存空闲时无压力）
docker build -q -f "$RP/crates/embedding/Dockerfile.runtime" \
    --build-arg EMBEDDING_BIN=.bin/embedding-bin \
    --build-arg MODEL_DIR=.bin/model/bge-small-zh-v1.5 \
    -t "$IMG" "$RP" >/dev/null
echo "[镜像] $IMG 已重建 (md5=$MD5)"

cd "$CD"
docker compose up -d embedding

# 重建核验：容器实际镜像 ID 必须等于刚组装的 tag（防 compose 配方漂移静默 no-op，
# 同 ship-server-binary.sh 既有教训）
CID="$(docker compose ps -q embedding)"
RUN_IMG="$(docker inspect -f '{{.Image}}' "$CID")"
if [ "$RUN_IMG" != "$(docker image inspect -f '{{.Id}}' "$IMG")" ]; then
    echo "[失败] 容器未运行新镜像（running=${RUN_IMG:0:19}）" >&2
    echo "       检查 $CD/docker-compose.yml 的 embedding.image 是否为 $IMG" >&2
    exit 1
fi
echo "[核验] 容器已运行新镜像 $IMG (${RUN_IMG:0:19})"

if [ -n "$SKIP" ]; then exit 0; fi
echo "[验证] 等 health 通过 (timeout ${TIMEOUT}s)"
pass=0
for i in $(seq 1 "$TIMEOUT"); do
    if docker exec "$CID" curl -sf -m 3 http://localhost:23350/api/health >/tmp/emb-health.json 2>/dev/null; then
        echo "[通过] $(cat /tmp/emb-health.json)"
        pass=1; break
    fi
    sleep 1
done
[ "$pass" = "1" ] || { echo "[失败] health 未通过"; docker logs --tail 20 "$CID" >&2 || true; exit 1; }

echo "[冒烟] POST /api/embed"
EMB="$(docker exec "$CID" curl -sf -m 10 -X POST http://localhost:23350/api/embed \
    -H 'Content-Type: application/json' -d '{"text":"江湖测试"}' || true)"
echo "$EMB" | grep -q '"embedding"' \
    && echo "[冒烟通过] $(echo "$EMB" | head -c 120)..." \
    || { echo "[失败] embed 响应异常: $EMB" >&2; exit 1; }
REMOTE

echo "[完成] embedding 部署成功 (md5=${BIN_HASH:-unknown})"
echo "  next: cd $COMPOSE_DIR && docker compose up -d   # 如需重建 agent 容器使其接入"
