# ============================================================================
# Cyber-Jianghu Agent Dockerfile
# 多阶段构建 + BuildKit 缓存优化
# ============================================================================
# 启用 BuildKit: DOCKER_BUILDKIT=1 docker build ...
# 或在 docker-compose.yml 中设置: DOCKER_BUILDKIT=1
# ============================================================================

# ============================================================================
# Stage 1: 构建阶段 - 编译Rust代码
# ============================================================================
FROM rust:trixie AS builder

# 配置 crates.io sparse 协议（更稳定）
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

# 配置 Debian 镜像源（阿里云）
RUN sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list.d/debian.sources || \
    sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list

ENV RUSTUP_DIST_SERVER=https://mirrors.aliyun.com/rustup
ENV RUSTUP_UPDATE_ROOT=https://mirrors.aliyun.com/rustup
ENV CARGO_REGISTRIES_ALIYUN_INDEX=https://mirrors.aliyun.com/crates.io-index/ \
    CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

# 设置工作目录
WORKDIR /app

# 安装构建依赖（使用缓存挂载）
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# 配置 cargo 镜像源
RUN mkdir -p ./.cargo \
    && echo '[source.crates-io]\n\
replace-with = "aliyun"\n\
\n\
[source.aliyun]\n\
registry = "sparse+https://mirrors.aliyun.com/crates.io-index/"' > ./.cargo/config.toml

# 复制 Cargo 元数据和协议/Agent 源码（消除 placeholder 引起的 build cache 污染）
COPY Cargo.toml Cargo.lock* ./
COPY crates/protocol/Cargo.toml ./crates/protocol/
COPY crates/agent/Cargo.toml ./crates/agent/
COPY crates/embedding/Cargo.toml ./crates/embedding/
COPY crates/server/Cargo.toml ./crates/server/

# workspace 需要各成员至少存在有效 targets
RUN mkdir -p crates/server/src && echo "fn main() {}" > crates/server/src/main.rs && \
    mkdir -p crates/embedding/src && echo "" > crates/embedding/src/lib.rs

# 复制真实源码
COPY crates/protocol/src ./crates/protocol/src
COPY crates/embedding/src ./crates/embedding/src
COPY crates/agent/src ./crates/agent/src

# 构建（使用 BuildKit 缓存挂载加速依赖）
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    cargo build --release -p cyber-jianghu-agent && \
    cp /app/target/release/cyber-jianghu-agent /app/agent-bin

# ============================================================================
# Stage 2: 运行阶段 - 最小化镜像
# ============================================================================
FROM debian:trixie-slim

# 配置 Debian 镜像源（阿里云）
RUN sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list.d/debian.sources || \
    sed -i 's/deb.debian.org/mirrors.aliyun.com/g' /etc/apt/sources.list

# 安装运行时依赖
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# 创建非root用户
RUN groupadd -g 1000 cyberjianghu && \
    useradd -u 1000 -g cyberjianghu -m -s /bin/bash cyberjianghu

# 设置工作目录
WORKDIR /app

# 从构建阶段复制编译后的二进制文件
COPY --from=builder /app/agent-bin /app/agent

# 复制 entrypoint 脚本并设置权限
COPY crates/agent/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# 复制静态文件（Web 面板）
COPY crates/agent/static /app/static

# 创建数据和配置目录并设置权限
RUN mkdir -p /app/data /app/config && \
    chown -R cyberjianghu:cyberjianghu /app

# 暴露 HTTP API 端口（23340-23349）
# 默认使用 23340，可在运行时通过 PORT 环境变量覆盖
EXPOSE 23340

# 健康检查
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:23340/api/v1/health || exit 1

# 设置环境变量
ENV RUST_LOG=info \
    CYBER_JIANGHU_CONFIG_DIR=/app/config \
    CYBER_JIANGHU_DATA_DIR=/app/data

# 容器入口点
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]

# 默认启动（用于 OpenClaw 等外部助手集成）
CMD ["./agent", "run", "--port", "23340"]
