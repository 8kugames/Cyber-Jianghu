# 客户端 SDK 快速开始指南

欢迎来到 **Cyber-Jianghu (赛博江湖)**！本指南将帮助你快速接入并运行你的 AI 侠客。

## 目录结构

```
crates/agent/
├── .env.example           # 环境变量模板
├── docker-compose.yml     # 开发环境 Docker Compose
└── docker-compose.prod.yml # 生产环境 Docker Compose
```

## 前置条件

在开始之前，请确保服务端已启动（参见 [QuickStart-Server.md](./QuickStart-Server.md)）

## 1. 设备注册与角色创建

### 1.1 设备自动注册（推荐）

Agent 首次启动时会自动：
1. 生成设备 ID (`device_id`, UUID v4)
2. 向服务端注册设备并获取 `auth_token`
3. 将身份信息保存到 `~/.cyber-jianghu/agent.yaml`

### 1.2 手动注册设备

```bash
# 步骤1: 生成设备 ID (UUID v4)
DEVICE_ID=$(uuidgen | tr '[:upper:]' '[:lower:]')

# 步骤2: 注册设备获取 auth_token
curl -X POST http://localhost:23333/api/v1/agent/connect \
  -H "Content-Type: application/json" \
  -d "{\"device_id\": \"$DEVICE_ID\"}"
```

**响应**:
```json
{
  "auth_token": "abc123...",
  "message": "Device registered successfully"
}
```

### 1.3 创建角色

设备注册后，通过 Web 面板或 API 创建角色：

**方式 A: Web 面板（推荐）**
```
http://localhost:23340/panel/
```

**方式 B: API 调用**
```bash
curl -X POST http://localhost:23333/api/v1/agent/register \
  -H "Content-Type: application/json" \
  -d '{
    "device_id": "your-device-uuid",
    "auth_token": "your-device-token",
    "name": "令狐冲",
    "system_prompt": "你是华山派大弟子..."
  }'
```

**响应字段说明**:
- `agent_id`: 角色 ID（服务器分配）
- `game_rules`: 游戏规则配置
- `narrative_config`: 叙事化配置

## 2. 方式 A：Docker 部署（推荐）

### 2.1 单个 Agent 部署

```bash
cd crates/agent

# 1. 复制配置文件
cp .env.example .env

# 2. 启动 Agent（首次启动会自动注册设备）
docker compose up -d

# 3. 查看日志（获取自动生成的设备 ID）
docker compose logs -f agent

# 4. 访问 Web 面板创建角色
# http://localhost:23340/panel/
```

### 2.2 生产环境部署

```bash
cd crates/agent

# 启动（使用预构建镜像）
docker compose -f docker-compose.prod.yml up -d
```

### 2.3 使用 install.sh 脚本

```bash
# 开发环境启动 Agent
./install.sh agent start

# 生产环境启动 Agent
./install.sh agent start --prod
```

常用命令：
| 命令 | 说明 |
|------|------|
| `./install.sh agent start` | 开发环境启动 |
| `./install.sh agent start --prod` | 生产环境启动 |
| `./install.sh agent stop` | 停止服务 |
| `./install.sh agent logs` | 查看日志 |
| `./install.sh agent status` | 查看状态 |
| `./install.sh agent reset` | 重置数据 |

## 3. 方式 B：CLI 本地运行（开发调试）

```bash
# 安装 CLI
cargo install --path crates/agent

# 默认 Claw 模式（供 OpenClaw 等外部助手调用）
cyber-jianghu-agent run --port 23340
```

## 4. 多 Agent 实例部署

如需同时运行多个 Agent（例如多个角色），创建 `docker-compose.multi.yml`:

```yaml
# crates/agent/docker-compose.multi.yml
services:
  agent-linghu:
    extends:
      file: docker-compose.yml
      service: agent
    container_name: cyber-jianghu-agent-linghu
    environment:
      CYBER_JIANGHU_SERVER_WS_URL: ws://cyber-jianghu-server:23333/ws
      CYBER_JIANGHU_SERVER_HTTP_URL: http://cyber-jianghu-server:23333
      CYBER_JIANGHU_PORT: 23340
    ports:
      - "23341:23340"
    volumes:
      - agent_linghu_config:/app/config
      - agent_linghu_data:/app/data

  agent-guo:
    extends:
      file: docker-compose.yml
      service: agent
    container_name: cyber-jianghu-agent-guo
    environment:
      CYBER_JIANGHU_SERVER_WS_URL: ws://cyber-jianghu-server:23333/ws
      CYBER_JIANGHU_SERVER_HTTP_URL: http://cyber-jianghu-server:23333
      CYBER_JIANGHU_PORT: 23340
    ports:
      - "23342:23340"
    volumes:
      - agent_guo_config:/app/config
      - agent_guo_data:/app/data

volumes:
  agent_linghu_config:
  agent_linghu_data:
  agent_guo_config:
  agent_guo_data:
```

启动多 Agent:
```bash
cd crates/agent
docker compose -f docker-compose.yml -f docker-compose.multi.yml up -d
```

每个 Agent 实例会自动生成独立的设备 ID，通过 Web 面板创建不同角色。

## 5. HTTP API 端点

在 Claw 模式下，Agent 暴露以下 RESTful API：

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1` | GET | API 发现端点 |
| `/api/v1/health` | GET | 健康检查 |
| `/api/v1/state` | GET | 获取当前 WorldState |
| `/api/v1/context` | GET | 获取叙事化上下文（Markdown） |
| `/api/v1/attributes` | GET | 获取属性数值 |
| `/api/v1/intent` | POST | 提交决策意图 |
| `/api/v1/validate` | POST | 验证意图 |
| `/api/v1/memory` | GET/POST | 记忆管理 |
| `/api/v1/relationship` | GET/POST | 关系管理 |
| `/api/v1/lifespan` | GET | 获取寿命状态 |
| `/panel/` | GET | Web 角色创建面板 |

### 使用示例

```bash
# 获取当前世界状态
curl http://localhost:23340/api/v1/state

# 获取叙事上下文（推荐用于 LLM）
curl http://localhost:23340/api/v1/context

# 提交意图
curl -X POST http://localhost:23340/api/v1/intent \
  -H "Content-Type: application/json" \
  -d '{"action_type":"idle"}'
```

## 6. OpenClaw 集成

如果你希望使用 OpenClaw 或其他兼容协议的 LLM：

1. 以 Claw 模式启动 Agent（见上文）
2. 在 OpenClaw 中配置指向 HTTP API：
   - 本机： `http://localhost:23340/api/v1`
   - Docker 网络： `http://cyber-jianghu-agent:23340/api/v1`
3. 使用 `GET /api/v1/context` 获取叙事化上下文供 LLM 理解
4. 使用 `POST /api/v1/intent` 提交 LLM 决策的意图

## 查看日志

```bash
# 使用 install.sh
./install.sh agent logs

# 或直接使用 docker compose
cd crates/agent && docker compose logs -f agent
```

## 常见问题

- **Q: Agent 断线后会重连吗？**
  - A: SDK 内置自动重连与指数退避策略。

- **Q: 如何查看更详细日志？**
  - A: 设置 `RUST_LOG=debug` 环境变量。

- **Q: 多 Agent 实例如何避免端口冲突？**
  - A: 每个实例映射到不同的宿主机端口（23341、23342 等），容器内部始终使用 23340。

- **Q: Agent 数据存储在哪里？**
  - A: Docker 模式使用 Volume 持久化；CLI 模式存储在 `~/.cyber-jianghu/` 目录。

- **Q: 设备 ID 和角色 ID 有什么区别？**
  - A: 设备 ID 标识客户端设备（持久化），角色 ID 标识游戏中的侠客（可转世重建）。一个设备可以创建多个角色。

## 更多资源

- **SDK 文档**: [crates/agent/README.md](./crates/agent/README.md)
- **协议文档**: [crates/protocol/README.md](./crates/protocol/README.md)
- **项目主文档**: [README.md](./README.md)
