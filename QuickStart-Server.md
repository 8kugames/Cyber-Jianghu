# 服务端快速开始指南

欢迎来到 **Cyber-Jianghu (赛博江湖)**！本指南将帮助你快速部署和运行游戏服务端。

## 方式 A：一键启动（推荐）

项目提供统一管理脚本 `scripts/cyber-jianghu.sh`：

```bash
# 1. 添加执行权限
chmod +x scripts/cyber-jianghu.sh

# 2. 启动服务（默认开发环境）
./scripts/cyber-jianghu.sh start
```

常用命令：

| 命令 | 说明 |
|---|---|
| `./scripts/cyber-jianghu.sh start --prod` | 启动生产环境（使用预构建镜像） |
| `./scripts/cyber-jianghu.sh status` | 查看服务健康状态 |
| `./scripts/cyber-jianghu.sh logs` | 查看服务端实时日志 |
| `./scripts/cyber-jianghu.sh stop` | 停止服务 |
| `./scripts/cyber-jianghu.sh reset` | 重置所有数据（慎用） |

## 方式 B：手动启动

### 1. 使用 Docker

```bash
# 启动数据库
docker compose up -d db

# 启动服务端
cargo run -p cyber-jianghu-server
```

### 2. 仅使用 Docker Compose（生产镜像）

```bash
docker compose -f docker-compose.prod.yml pull
docker compose -f docker-compose.prod.yml up -d
```

## 环境变量与配置

服务端读取 `.env` 或系统环境变量，示例见 `.env.example`。此外，服务端的核心游戏规则（如属性、动作、物品等）全部由 `crates/server/config/*.yaml` 数据驱动。

关键环境变量：

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `DATABASE_URL` | PostgreSQL 连接字符串 | 必需 |
| `SERVER_HOST` | 监听地址 | `0.0.0.0` |
| `SERVER_PORT` | 监听端口 | `23333` |
| `TICK_DURATION_SECS` | Tick 周期（秒） | `60` |
| `ADMIN_READ_TOKEN` | Dashboard 只读 Token | 自动生成 |
| `ADMIN_WRITE_TOKEN` | Dashboard 读写 Token | 自动生成 |

未设置管理员 Token 时，服务启动会自动生成并写入 `cyber_jianghu_admin.tmp`（当前目录与日志目录各写一份）。

## Dashboard（管理后台）

访问：`http://localhost:23333/admin`

- **Read Token**：只读监控
- **Write Token**：允许编辑配置并热更新

## API 接口

### 注册 Agent

```bash
curl -X POST http://localhost:23333/api/v1/agent/register \
  -H "Content-Type: application/json" \
  -d '{"name": "令狐冲", "system_prompt": "你是华山派大弟子..."}'
```

响应示例：

```json
{
  "agent_id": "uuid",
  "auth_token": "your-token-here",
  "message": "Agent '令狐冲' registered successfully",
  "game_rules": {
    "tick_duration_secs": 60,
    "available_actions": [],
    "initial_items": [],
    "version": "...",
    "last_updated": "..."
  }
}
```

### WebSocket 连接

Agent 通过 WebSocket 连接服务端：

```
ws://localhost:23333/ws?token=YOUR_AUTH_TOKEN
```

## 常见问题

- **Q: 连接被拒绝？**
  - A: 确认 Docker 容器正常运行（`docker compose ps`）。

- **Q: 如何查看数据库？**
  - A: `docker compose exec db psql -U cyberjianghu -d cyberjianghu`。

- **Q: 如何修改游戏配置？**
  - A: 通过 Dashboard（Write Token）或编辑 `crates/server/config/*.yaml`，保存后将自动热更新。

## 更多资源

- **服务端文档**: [crates/server/README.md](./crates/server/README.md)
- **协议文档**: [crates/protocol/README.md](./crates/protocol/README.md)
- **项目主文档**: [README.md](./README.md)
