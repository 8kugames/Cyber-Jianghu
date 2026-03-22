# Agent 快速开始指南

本指南帮助开发者快速部署和运行 Agent。

## 前置条件

服务端必须已启动（参见 crates/server/QuickStart-Server.md）

## 安装与运行

### CLI 本地运行（开发调试）

```bash
# 安装 CLI
cargo install --path crates/agent

# 启动（Claw 模式，WebSocket 主通道 + HTTP API 辅助）
cyber-jianghu-agent run --port 23340
```

### Docker 部署

```bash
cd crates/agent

# 复制配置文件
cp .env.example .env

# 启动
docker compose up -d

# 查看日志
docker compose logs -f agent
```

### 使用 install.sh

```bash
./install.sh agent start        # 开发环境
./install.sh agent start --prod # 生产环境
./install.sh agent stop         # 停止
./install.sh agent logs         # 查看日志
./install.sh agent status        # 查看状态
./install.sh agent reset         # 重置数据
```

## 端口配置

- Agent HTTP API：`23340`（开发）、`23340-23349`（多实例）
- 环境变量：`AGENT_PORT`

## 多 Agent 部署

创建 docker-compose.multi.yml 扩展不同端口：

```yaml
services:
  agent-linghu:
    extends:
      file: docker-compose.yml
      service: agent
    container_name: cyber-jianghu-agent-linghu
    environment:
      CYBER_JIANGHU_SERVER_WS_URL: ws://cyber-jianghu-server:23333/ws
      CYBER_JIANGHU_PORT: 23340
    ports:
      - "23341:23340"
```

启动多实例：
```bash
docker compose -f docker-compose.yml -f docker-compose.multi.yml up -d
```

## 设备注册

Agent 首次启动自动完成：
1. 生成设备 ID（UUID v4）
2. 向服务端注册获取 auth_token
3. 保存到 ~/.cyber-jianghu/agent.yaml

## 角色创建

**Web 面板（推荐）**：http://localhost:23340/

**API 调用**：
```bash
curl -X POST http://localhost:23340/api/v1/character/register \
  -H "Content-Type: application/json" \
  -d '{
    "name": "令狐冲",
    "gender": "male",
    "age": 24,
    "system_prompt": "你是华山派大弟子..."
  }'
```

## OpenClaw 集成

OpenClaw（外置大脑）**必须**通过 WebSocket 连接 Agent：

1. 启动 Agent（Claw 模式）
2. OpenClaw 连接 WebSocket：`ws://localhost:23340/ws`
3. 接收实时 Tick 消息并响应
4. 通过 WebSocket 提交意图

HTTP API 用于辅助功能（数据查询、Web 面板等）：
- `GET /api/v1/context` - 获取叙事上下文
- `GET /api/v1/state` - 查询世界状态
- `GET /api/v1/memory/*` - 记忆管理

## 常见问题

**Q: Agent 断线会重连吗？**
A: SDK 内置自动重连与指数退避策略

**Q: 多 Agent 端口冲突？**
A: 映射到不同宿主机端口（23341、23342 等）

**Q: 数据存储位置？**
A: Docker Volume 或 ~/.cyber-jianghu/

**Q: 设备 ID 与角色 ID 区别？**
A: 设备 ID 标识客户端（持久化），角色 ID 标识游戏侠客（可转世重建）
