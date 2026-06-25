# 服务端快速开始指南

> **English version**: [QuickStart-Server.en.md](./QuickStart-Server.en.md)

本指南帮助开发者快速部署和运行 Cyber-Jianghu 游戏服务端。

## 环境要求

- Rust (Cargo)
- Docker 和 Docker Compose
- PostgreSQL（可以通过 Docker 容器运行）

## 启动服务

### 使用根目录的 install.sh（推荐）

在项目**根目录**下使用提供的安装脚本管理服务：

```bash
# 开发环境启动（同时启动 Server 和 Agent）
./install.sh all start

# 生产环境启动
./install.sh all start --prod

# 查看服务状态
./install.sh all status

# 查看日志
./install.sh all logs

# 停止服务
./install.sh all stop

# 警告：重置所有数据库和卷数据
./install.sh all reset
```

### 纯本地 Cargo 启动（用于开发调试）

确保数据库已经启动（例如通过 `docker compose up -d db`），并且已经在 `crates/server/.env` 中配置好数据库连接。

```bash
cd crates/server

# 复制环境变量模板并修改数据库密码/连接串
cp .env.example .env

# 运行服务器
cargo run

# 或以 Release 模式运行
cargo run --release
```

## 环境变量配置

服务端启动时会加载 `crates/server/.env`。主要变量说明：

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `DATABASE_URL` | PostgreSQL 连接字符串 | 无 |
| `SERVER_HOST` | 服务端绑定 IP | `0.0.0.0` |
| `SERVER_PORT` | 服务端监听端口 | `23333` |
| `ADMIN_READ_TOKEN` | Dashboard 只读 Token | 未配置则自动生成 |
| `ADMIN_WRITE_TOKEN` | Dashboard 读写 Token | 未配置则自动生成 |
| `RUST_LOG` | 日志级别 | `info` |

> **提示**：Tick 周期已不再通过环境变量配置，请修改 `config/game_rules.yaml` 中的 `tick.real_seconds_per_tick` 字段。

## 服务访问

- **API 基础地址**：`http://localhost:23333`
- **管理面板 (Dashboard)**：`http://localhost:23333/admin/`
  - 如果未在环境变量配置 Token，服务器启动时会自动生成并写入到 `crates/server/logs/cyber_jianghu_admin.tmp` 文件中。
- **健康检查**：`http://localhost:23333/health`
- **WebSocket 接入点**：`ws://localhost:23333/ws?token=YOUR_AUTH_TOKEN`

## 管理与配置

1. **热重载配置**：可以通过 Dashboard 发起配置热重载，或者通过 `POST /api/admin/reload-config`。
2. **游戏数据配置**：位于 `crates/server/config/*.yaml`，可以直接编辑文件或通过 Dashboard 的编辑器进行修改。
