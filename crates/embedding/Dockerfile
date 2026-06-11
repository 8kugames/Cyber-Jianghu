# ============================================================================
# Cyber-Jianghu Embedding Service Dockerfile
# 独立向量嵌入 HTTP 服务（bge-small-zh-v1.5）
# ============================================================================

# ============================================================================
# Stage 1: 构建
# ============================================================================
FROM rust:trixie AS builder

ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

RUN sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list.d/debian.sources || \
    sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list

ENV RUSTUP_DIST_SERVER=https://mirrors.aliyun.com/rustup
ENV RUSTUP_UPDATE_ROOT=https://mirrors.aliyun.com/rustup
ENV CARGO_REGISTRIES_ALIYUN_INDEX=https://mirrors.aliyun.com/crates.io-index/ \
    CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

WORKDIR /app

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p ./.cargo \
    && echo '[source.crates-io]\n\
replace-with = "aliyun"\n\
\n\
[source.aliyun]\n\
registry = "sparse+https://mirrors.aliyun.com/crates.io-index/"' > ./.cargo/config.toml

COPY Cargo.toml Cargo.lock* ./
COPY crates/protocol/Cargo.toml ./crates/protocol/
COPY crates/embedding/Cargo.toml ./crates/embedding/
COPY crates/server/Cargo.toml ./crates/server/

RUN mkdir -p crates/server/src && echo "fn main() {}" > crates/server/src/main.rs
RUN mkdir -p crates/agent/src && echo "fn main() {}" > crates/agent/src/main.rs

COPY crates/protocol/src ./crates/protocol/src
COPY crates/embedding/src ./crates/embedding/src

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build --release -p cyber-jianghu-embedding && \
    cp /app/target/release/cyber-jianghu-embedding /app/embedding-bin

# ============================================================================
# Stage 2: 运行
# ============================================================================
FROM debian:trixie-slim

RUN sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list.d/debian.sources || \
    sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -g 1000 cyberjianghu && \
    useradd -u 1000 -g cyberjianghu -m -s /bin/bash cyberjianghu

WORKDIR /app

COPY --from=builder /app/embedding-bin /app/embedding-service

RUN mkdir -p /app/data /app/config && \
    chown -R cyberjianghu:cyberjianghu /app

ARG HF_MIRROR=https://hf-mirror.com
ARG MODEL_DIR=/app/data/models/BAAI/bge-small-zh-v1.5
RUN mkdir -p ${MODEL_DIR} && \
    for FILE in config.json tokenizer.json model.safetensors; do \
        curl -fsSL "${HF_MIRROR}/BAAI/bge-small-zh-v1.5/resolve/main/${FILE}" \
            -o ${MODEL_DIR}/${FILE} && \
        SHA256_FILE=$(curl -fsSL "${HF_MIRROR}/BAAI/bge-small-zh-v1.5/resolve/main/${FILE}.sha256" 2>/dev/null || echo "") && \
        if [ -n "$SHA256_FILE" ]; then \
            EXPECTED=$(echo "$SHA256_FILE" | awk '{print $1}'); \
            ACTUAL=$(sha256sum ${MODEL_DIR}/${FILE} | awk '{print $1}'); \
            if [ "$EXPECTED" != "$ACTUAL" ]; then \
                echo "SHA256 mismatch for ${FILE}: expected ${EXPECTED}, got ${ACTUAL}"; \
                exit 1; \
            fi; \
            echo "SHA256 verified: ${FILE}"; \
        else \
            echo "SHA256 not available for ${FILE}, skipping verification"; \
        fi; \
    done && \
    chown -R cyberjianghu:cyberjianghu /app/data

EXPOSE 23350

HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:23350/api/health || exit 1

ENV RUST_LOG=info \
    CYBER_JIANGHU_DATA_DIR=/app/data \
    EMBEDDING_PORT=23350

USER cyberjianghu

CMD ["./embedding-service"]
