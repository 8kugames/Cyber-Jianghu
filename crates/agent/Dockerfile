# ============================================================================
# Cyber-Jianghu Agent Dockerfile
# 多阶段构建 - 优化镜像大小和构建速度
# ============================================================================

# ============================================================================
# Stage 1: 构建阶段 - 编译Rust代码
# ============================================================================
FROM rust:bookworm AS builder

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

# 安装构建依赖
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# 优化构建缓存：先复制 Cargo 文件（包含所有 workspace 成员）
COPY Cargo.toml ./
COPY crates/protocol/Cargo.toml ./crates/protocol/
COPY crates/server/Cargo.toml ./crates/server/
COPY crates/agent/Cargo.toml ./crates/agent/

RUN mkdir -p ./.cargo \
    && echo '[source.crates-io]\n\
replace-with = "aliyun"\n\
\n\
[source.aliyun]\n\
registry = "sparse+https://mirrors.aliyun.com/crates.io-index/"' > ./.cargo/config.toml

# 创建空的 src 目录，用于下载依赖
RUN mkdir -p crates/protocol/src crates/server/src crates/agent/src/bin && \
    echo "pub fn placeholder() {}" > crates/protocol/src/lib.rs && \
    echo "pub fn placeholder() {}" > crates/server/src/lib.rs && \
    echo "pub fn placeholder() {}" > crates/agent/src/lib.rs && \
    echo "fn main() {}" > crates/agent/src/bin/cyber-jianghu-agent.rs

# 构建依赖（这一步会被缓存）
RUN cargo build --release -p cyber-jianghu-agent

# 复制实际源代码
COPY crates/protocol/src ./crates/protocol/src
COPY crates/agent/src ./crates/agent/src

# 重新构建（只编译源代码，依赖已缓存）
RUN touch crates/protocol/src/lib.rs && \
    cargo build --release -p cyber-jianghu-agent

# ============================================================================
# Stage 2: 运行阶段 - 最小化镜像
# ============================================================================
FROM debian:bookworm-slim

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
COPY --from=builder /app/target/release/cyber-jianghu-agent /app/agent

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

# 设置环境变量
ENV RUST_LOG=info \
    CYBER_JIANGHU_CONFIG_DIR=/app/config \
    CYBER_JIANGHU_DATA_DIR=/app/data

# 容器入口点
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]

# 默认启动（用于 OpenClaw 等外部助手集成）
CMD ["./agent", "run", "--port", "23340"]
