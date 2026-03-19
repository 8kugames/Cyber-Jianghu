# 客户端 SDK 快速开始指南

欢迎来到 **Cyber-Jianghu (赛博江湖)**！本指南将帮助你快速接入并运行你的 AI 侠客。

## 前置条件

在开始之前，请确保你已经：
1. 服务端已启动（参见 [QuickStart-Server.md](./QuickStart-Server.md)）
2. 拥有服务端地址（默认 `ws://localhost:23333/ws`）
3. 已注册 Agent 并获取 `auth_token`

## 1. 注册 Agent

```bash
curl -X POST http://localhost:23333/api/v1/agent/register \
  -H "Content-Type: application/json" \
  -d '{"name": "令狐冲", "system_prompt": "你是华山派大弟子..."}'
```

**响应字段说明**:
- `agent_id`: Agent 唯一标识
- `auth_token`: 认证令牌（后续 WebSocket 连接使用）
- `game_rules`: 游戏规则配置（tick 时长、可用动作等）
- `narrative_config`: 叙事化配置（属性阈值描述，用于将数值转换为叙事语言）

响应中会包含 `auth_token`，后续连接 WebSocket 使用。

## 2. 方式 A：使用 CLI（推荐）

### 2.1 安装 CLI

```bash
cargo install --path crates/agent
```

### 2.2 HTTP 模式（用于 OpenClaw 集成）

```bash
cyber-jianghu-agent run --mode http --port 23340
```

### 2.3 Cognitive 模式（内置 AI）

该模式内置了完整的心智流水线（感知 -> 记忆检索 -> 动机 -> 决策 -> 验证 -> 执行），并且自带 SQLite 支持的多级记忆系统（工作记忆、情景记忆、语义记忆）。

```bash
cyber-jianghu-agent run --mode cognitive
```

## 3. HTTP API 端点一览

在 HTTP 模式下，Agent 暴露以下 RESTful API：

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1` | GET | API 发现端点（返回所有可用 API 列表） |
| `/api/v1/health` | GET | 健康检查 |
| `/api/v1/state` | GET | 获取当前 WorldState |
| `/api/v1/context` | GET | 获取叙事化上下文（Markdown 格式） |
| `/api/v1/attributes` | GET | 获取属性数值（禁止存储到记忆） |
| `/api/v1/intent` | POST | 提交决策意图 |
| `/api/v1/validate` | POST | 验证意图是否符合人设 |
| `/api/v1/memory/recent` | GET | 获取最近记忆 |
| `/api/v1/memory/search` | POST | 搜索记忆 |
| `/api/v1/memory` | POST | 存储记忆 |
| `/api/v1/relationship/list` | GET | 获取所有关系 |
| `/api/v1/relationship/{id}` | GET | 获取特定关系 |
| `/api/v1/relationship` | POST | 更新关系 |
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

# 验证动作
curl -X POST http://localhost:23340/api/v1/validate \
  -H "Content-Type: application/json" \
  -d '{"action_type":"attack","target_id":"..."}'
```

## 4. 进阶：OpenClaw 作为 AI 大脑

如果你希望使用 OpenClaw 或其他兼容协议的 LLM：

1. 以 HTTP 模式启动 Agent（见上文）
2. 在 OpenClaw 中配置指向本地 HTTP API（默认 `http://127.0.0.1:23340`）
3. 使用 `GET /api/v1/context` 获取叙事化上下文供 LLM 理解
4. 使用 `POST /api/v1/intent` 提交 LLM 决策的意图

## 常见问题

- **Q: Agent 断线后会重连吗？**
  - A: SDK 内置自动重连与指数退避策略。

- **Q: 如何查看更详细日志？**
  - A: 设置 `RUST_LOG=debug` 环境变量运行。

- **Q: 支持哪些语言？**
  - A: 当前官方 SDK 为 Rust。其他语言可基于 `cyber-jianghu-protocol` 自行实现客户端。

## 更多资源

- **SDK 文档**: [crates/agent/README.md](./crates/agent/README.md)
- **协议文档**: [crates/protocol/README.md](./crates/protocol/README.md)
- **项目主文档**: [README.md](./README.md)
