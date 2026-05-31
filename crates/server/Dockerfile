# ============================================================================
# OpenClaw Cyber-Jianghu MVP Server Dockerfile
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
    libpq-dev \
    && rm -rf /var/lib/apt/lists/*

# 配置 cargo 镜像源
RUN mkdir -p ./.cargo \
    && echo '[source.crates-io]\n\
    replace-with = "aliyun"\n\
    \n\
    [source.aliyun]\n\
    registry = "sparse+https://mirrors.aliyun.com/crates.io-index/"' > ./.cargo/config.toml

# 优化构建缓存：先复制 Cargo 文件（包含所有 workspace 成员）
# Cargo.lock 可选（如果存在则使用 --locked，否则正常构建）
COPY Cargo.toml Cargo.lock* ./
COPY crates/protocol/Cargo.toml ./crates/protocol/
COPY crates/server/Cargo.toml ./crates/server/
COPY crates/agent/Cargo.toml ./crates/agent/

# 创建空的 src 目录，用于下载依赖（仅 server 需要的依赖）
RUN mkdir -p crates/protocol/src crates/server/src crates/agent/src/bin && \
    echo "pub fn placeholder() {}" > crates/protocol/src/lib.rs && \
    echo "fn main() {}" > crates/server/src/main.rs && \
    echo "pub fn placeholder() {}" > crates/agent/src/lib.rs && \
    echo "fn main() {}" > crates/agent/src/bin/cyber-jianghu-agent.rs

# 构建依赖（使用 BuildKit 缓存挂载，依赖会被持久化）
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    if [ -f Cargo.lock ]; then \
        cargo build --release -p cyber-jianghu-server; \
    else \
        cargo build --release -p cyber-jianghu-server; \
    fi

# 复制实际源代码（server 仅依赖 protocol）
COPY crates/protocol/src ./crates/protocol/src
COPY crates/server/src ./crates/server/src

# 重新构建（只编译源代码，依赖已缓存）
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/app/target,sharing=locked \
    if [ -f Cargo.lock ]; then \
        cargo build --release -p cyber-jianghu-server; \
    else \
        cargo build --release -p cyber-jianghu-server; \
    fi && \
    cp /app/target/release/cyber-jianghu-server /app/server-bin

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
    libpq5 \
    postgresql-client \
    && rm -rf /var/lib/apt/lists/*

# 创建非root用户
RUN groupadd -g 1000 cyberjianghu && \
    useradd -u 1000 -g cyberjianghu -m -s /bin/bash cyberjianghu

# 设置工作目录
WORKDIR /app

# 从构建阶段复制编译后的二进制文件
COPY --from=builder /app/server-bin /app/server

# 复制默认配置文件到备用位置（数据驱动架构 - 支持降级）
# 运行时：
#   1. 如果 /app/config 通过 volume 挂载了外部配置 → 使用外部配置
#   2. 如果 /app/config 为空 → 从 /app/default_config 自动填充
COPY crates/server/config /app/default_config

# 复制静态文件（Dashboard 管理面板）
COPY crates/server/static /app/static

# 复制数据库迁移文件
COPY crates/server/migrations/*.sql /app/migrations/

# 复制 entrypoint 脚本并设置权限
COPY crates/server/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# 创建日志目录并设置权限
RUN mkdir -p /app/logs && \
    chown -R cyberjianghu:cyberjianghu /app

# 暴露端口
EXPOSE 23333

# 设置环境变量
ENV RUST_LOG=info \
    CYBER_JIANGHU_CONFIG_DIR=/app/config \
    CYBER_JIANGHU_STATIC_DIR=/app/static \
    CYBER_JIANGHU_LOGS_DIR=/app/logs \
    CYBER_JIANGHU_DATA_DIR=/app/data

# 容器入口点（以 root 运行来修复权限，然后切换用户执行）
# 注意：容器以 root 启动，entrypoint 会检查权限并切换到 cyberjianghu 用户运行服务
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]

# 健康检查（会通过 entrypoint 以正确的用户运行）
HEALTHCHECK --interval=30s --timeout=10s --retries=3 --start-period=40s \
    CMD curl -f http://localhost:23333/health || exit 1

# 启动服务
CMD ["./server"]
