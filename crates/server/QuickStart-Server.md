# 服务端快速开始指南

本指南帮助开发者快速部署和运行游戏服务端。

## 环境要求

- Docker 和 Docker Compose
- PostgreSQL（Docker 容器内运行）

## 安装与运行

### 使用 install.sh（推荐）

```bash
./install.sh server start        # 开发环境
./install.sh server start --prod # 生产环境
./install.sh server stop         # 停止
./install.sh server restart      # 重启
./install.sh server status       # 查看状态
./install.sh server logs        # 查看日志
./install.sh server build        # 构建镜像
./install.sh server reset        # 重置数据
```

### 使用 Docker Compose

```bash
cd crates/server

# 复制配置文件
cp .env.example .env

# 启动
docker compose up -d

# 查看日志
docker compose logs -f server
```

## 环境变量配置

| 变量 | 说明 | 默认值 |
|------|------|--------|
| DB_PASSWORD | PostgreSQL 密码（必须修改） | changeme |
| POSTGRES_PORT | PostgreSQL 端口 | 5432 |
| SERVER_PORT | 服务端端口 | 23333 |
| ADMIN_READ_TOKEN | Dashboard 只读 Token | 自动生成 |
| ADMIN_WRITE_TOKEN | Dashboard 读写 Token | 自动生成 |
| RUST_LOG | 日志级别 | info |
| ENVIRONMENT | 环境标识 | development |

> **注意**：`TICK_DURATION_SECS` 环境变量已被废弃。Tick 周期通过 `config/game_rules.yaml` 的 `tick.real_seconds_per_tick` 配置。

## 访问服务

- **服务端**：`http://localhost:23333`
- **Dashboard**：`http://localhost:23333/admin`
  - Read Token：只读监控
  - Write Token：编辑配置并热更新
  - Token 未设置时，服务启动自动生成并写入日志

## WebSocket 连接

```
ws://localhost:23333/ws?token=YOUR_AUTH_TOKEN
```

## 数据库访问

```bash
docker compose exec postgres psql -U postgres -d cyber_jianghu
```

## 配置热重载

通过 Dashboard（Write Token）或重启服务生效

## 常见问题

**Q: 连接被拒绝？**
A: 确认容器正常运行 `docker compose ps`

**Q: 如何修改游戏配置？**
A: Dashboard（Write Token）或编辑 `crates/server/config/*.yaml`

**Q: Agent 部署？**
A: 参见 crates/agent/QuickStart-Agent.md
