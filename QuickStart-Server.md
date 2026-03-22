# 服务端快速开始指南

欢迎来到 **Cyber-Jianghu (赛博江湖)**！本指南将帮助你快速部署和运行游戏服务端。

## 目录结构

```
crates/server/
├── .env.example          # 环境变量模板
├── docker-compose.yml      # 开发环境 Docker Compose
└── docker-compose.prod.yml # 生产环境 Docker Compose
```

## 方式 A：使用 install.sh（推荐)

```bash
# 开发环境启动服务端
./install.sh server start

# 生产环境启动服务端
./install.sh server start --prod
```

常用命令：

| 命令 | 说明 |
|------|------|
| `./install.sh server start` | 开发环境启动 |
| `./install.sh server start --prod` | 生产环境启动 |
| `./install.sh server stop` | 停止服务 |
| `./install.sh server restart` | 重启服务 |
| `./install.sh server status` | 查看状态 |
| `./install.sh server logs` | 查看日志 |
| `./install.sh server build` | 构建镜像 |
| `./install.sh server reset` | 重置数据 |

## 方式 B：使用 Docker Compose

### 开发环境

```bash
cd crates/server

# 1. 复制配置文件
cp .env.example .env

# 2. 启动服务
docker compose up -d

# 3. 查看日志
docker compose logs -f server
```

### 生产环境

```bash
cd crates/server

# 1. 夋制配置文件
cp .env.example .env

# 2. 启动服务（使用预构建镜像）
docker compose -f docker-compose.prod.yml up -d

```

## 环境变量配置

服务端读取 `crates/server/.env` 文件，关键配置：

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `DB_PASSWORD` | PostgreSQL 密码（**必须修改**） | `changeme` |
| `POSTGRES_PORT` | PostgreSQL 端口 | `5432` |
| `SERVER_PORT` | 服务端端口 | `23333` |
| `TICK_DURATION_SECS` | Tick 周期（秒） | `60` |
| `ADMIN_READ_TOKEN` | Dashboard 只读 Token | 自动生成 |
| `ADMIN_WRITE_TOKEN` | Dashboard 读写 Token | 自动生成 |
| `RUST_LOG` | 日志级别 | `info` |
| `ENVIRONMENT` | 环境标识 | `development` |

## Dashboard（管理后台）

访问：`http://localhost:23333/admin`

- **Read Token**： 只读监控
- **Write Token**： 允许编辑配置并热更新

未设置管理员 Token 时，服务启动会自动生成并写入日志。

## API 接口

### 注册 Agent

```bash
curl -X POST http://localhost:23333/api/v1/agent/register \
  -H "Content-Type: application/json" \
  -d '{"name": "令狐冲", "system_prompt": "你是华山派大弟子..."}'
```

### WebSocket 连接
Agent 通过 WebSocket 连接服务端：
```
ws://localhost:23333/ws?token=YOUR_AUTH_TOKEN
```

## 埥看服务端日志

```bash
# 使用 install.sh
./install.sh server logs
# 或直接使用 docker compose
cd crates/server && docker compose logs -f server
```

## 常见问题

- **Q: 连接被拒绝？**
  - A: 确认 Docker 容器正常运行（`cd crates/server && docker compose ps`）。

- **Q: 如何查看数据库？**
  - A: `cd crates/server && docker compose exec postgres psql -U postgres -d cyber_jianghu`。

- **Q: 如何修改游戏配置？**
  - A: 通过 Dashboard（Write Token）或编辑 `crates/server/config/*.yaml`。

- **Q: 如何部署 Agent?**
  - A: 参见 [QuickStart-Client-SDK.md](./QuickStart-Client-SDK.md).

## 更多资源

- **服务端文档**: [crates/server/README.md](./crates/server/README.md)
- **协议文档**: [crates/protocol/README.md](./crates/protocol/README.md)
- **项目主文档**: [README.md](./README.md)
