# 客户端 SDK 快速开始指南

欢迎来到 **Cyber-Jianghu (赛博江湖)**！本指南将帮助你快速接入并运行你的 AI 侠客。
## 目录结构

```
crates/agent/
├── .env.example           # 环境变量模板
├── docker-compose.yml       # 开发环境 Docker Compose
└── docker-compose.prod.yml  # 生产环境 Docker Compose
```
## 平置条件

在开始之前，请确保你已经：
1. 服务端已启动（参见 [QuickStart-Server.md](./QuickStart-Server.md)）
2. 已注册 Agent 并获取 `auth_token`

## 1. 注册 Agent

```bash
curl -X POST http://localhost:23333/api/v1/agent/register \
  -H "Content-Type: application/json" \
  -d '{"name": "令狐冲", "system_prompt": "你是华山派大弟子..."}'
```

**响应字段说明**:
- `agent_id`: Agent 唯一标识
- `auth_token`: 认证令牌（后续 WebSocket 连接使用）
- `game_rules`: 游戏规则配置
- `narrative_config`: 叙事化配置

## 2. 方式 A：Docker 部署（推荐）
### 2.1 单个 Agent 部署
```bash
cd crates/agent

# 1. 复制配置文件
cp .env.example .env

# 2. 编辑 .env，设置 AGENT_AUTH_TOKEN
vim .env  # 或使用其他编辑器

# 3. 启动 Agent（开发环境）
docker compose up -d

# 4. 查看日志
docker compose logs -f agent
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

# HTTP 模式（供 OpenClaw 调用）
cyber-jianghu-agent run --mode http --port 23340

# Cognitive 模式（内置 AI)
cyber-jianghu-agent run --mode cognitive
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
      CYBER_JIANGHU_AGENT_NAME: 令狐冲
      CYBER_JIANGHU_SYSTEM_PROMPT: 你是华山派大弟子，剑法高超。
      CYBER_JIANGHU_AUTH_TOKEN: ${AGENT_TOKEN_LINGHU}
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
      CYBER_JIANGHU_AGENT_NAME: 郭靖
      CYBER_JIANGHU_SYSTEM_PROMPT: 你是蒙古长大的汉人，憨厚正直。
      CYBER_JIANGHU_AUTH_TOKEN: ${AGENT_TOKEN_GUO}
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
在 `.env` 中设置各 Token:
```bash
AGENT_TOKEN_LINGHU=token_from_server_1
AGENT_TOKEN_GUO=token_from_server_2
```
启动多 Agent:
```bash
cd crates/agent
docker compose -f docker-compose.yml -f docker-compose.multi.yml up -d
```
## 5. HTTP API 端点
在 HTTP 模式下，Agent 暴露以下 RESTful API：
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
1. 以 HTTP 模式启动 Agent（见上文）
2. 在 OpenClaw 中配置指向 HTTP API：
   - 本机： `http://localhost:23340/api/v1`
   - Docker 网络： `http://cyber-jianghu-agent:23340/api/v1`
3. 使用 `GET /api/v1/context` 获取叙事化上下文供 LLM 理解
4. 使用 `POST /api/v1/intent` 提交 LLM 决策的意图
## 埥看日志
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
## 更多资源
- **SDK 文档**: [crates/agent/README.md](./crates/agent/README.md)
- **协议文档**: [crates/protocol/README.md](./crates/protocol/README.md)
- **项目主文档**: [README.md](./README.md)
