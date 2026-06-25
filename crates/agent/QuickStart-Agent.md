# Agent 快速开始指南

> **English version**: [QuickStart-Agent.en.md](./QuickStart-Agent.en.md)

本指南帮助开发者快速部署和运行 Agent SDK。

## 前置条件

由于 Agent 是连接服务端的桥梁，**服务端必须已启动**（参见 `crates/server/QuickStart-Server.md`）。

## 运行模式与启动

Agent 核心逻辑（认知流转、三层记忆、多重人格）均在同一套架构下运行。唯一的区别是 LLM（大语言模型）的调用位置：

| 模式 | 描述 | LLM 客户端实现 | 启动命令 |
|------|------|---------------|----------|
| **Cognitive** (默认) | 完全自治。Agent 内部直接调用大模型，闭环生成 Intent。 | `FallbackLlmClient` | `cyber-jianghu-agent run` |
| **Claw** | 外部大脑。通过 OpenClaw 桥接外部大模型，Agent 本身只提供上下文，决策由 OpenClaw 注入。 | `OpenClawBridge` | `cyber-jianghu-agent run --mode claw` |

## 安装与部署

### 1. 本地开发 (CLI)

```bash
# 从源码安装 CLI
cargo install --path crates/agent

# 启动默认模式
cyber-jianghu-agent run

# 或者指定端口启动
CYBER_JIANGHU_PORT=23340 cyber-jianghu-agent run
```

### 2. Docker 部署

```bash
cd crates/agent

# 配置环境变量
cp .env.example .env

# 启动容器
docker compose up -d

# 查看日志
docker compose logs -f agent
```

### 3. 一键脚本

```bash
./install.sh agent start        # 启动 Agent
./install.sh agent logs         # 实时查看日志
./install.sh agent stop         # 停止服务
./install.sh agent reset        # 重置所有本地数据
```

## 网络与端口

Agent 会启动一个本地的 HTTP API 服务，用于管理面板、状态查询以及与 OpenClaw（在 Claw 模式下）的通信。

- **默认端口范围**：`23340-23999`
- **指定端口**：设置环境变量 `CYBER_JIANGHU_PORT=23340`。若设置为 `0` 或未设置，Agent 会自动在范围内分配可用端口。
- **服务端连接**：必须确保 `CYBER_JIANGHU_SERVER_WS_URL` 正确指向服务端的 WebSocket 端点（如 `ws://localhost:23333/ws`）。

## 多 Agent 部署 (设备与角色分离)

Agent SDK 支持在同一个设备（进程）上托管多个角色。同时，也支持启动多个 Agent 进程，分别映射到不同端口。

```yaml
# docker-compose.multi.yml 示例
services:
  agent-linghu:
    extends:
      file: docker-compose.yml
      service: agent
    container_name: cyber-jianghu-agent-linghu
    environment:
      CYBER_JIANGHU_PORT: 23340
    ports:
      - "23340:23340"

  agent-renwoxing:
    extends:
      file: docker-compose.yml
      service: agent
    container_name: cyber-jianghu-agent-renwoxing
    environment:
      CYBER_JIANGHU_PORT: 23341
    ports:
      - "23341:23341"
```

## 核心工作流：如何交互

Agent 内部**强制使用 WebSocket** 向 Server 提交 Intent。 HTTP API 只作为辅助。

1. **设备注册**：首次启动时自动生成 UUID v4 作为 Device ID，并向 Server 注册。
2. **角色创建**：
   ```bash
   curl -X POST http://localhost:23340/api/v1/character/register \
     -H "Content-Type: application/json" \
     -d '{
       "name": "令狐冲",
       "gender": "male",
       "age": 24,
       "system_prompt": "你是华山派大弟子，生性豁达..."
     }'
   ```
3. **Tick 循环**：服务端每 N 秒下发 `WorldState`，Agent 接收后触发 `CognitiveEngine`（人魂），通过 LLM 推理生成 Intent。
4. **验证与执行**：生成的 Intent 先经过 `ReflectorSoul`（天魂）的三层规则审查，通过后提交到服务端的 `IntentWorker`。

## 配置管理 (Agent.yaml)

Agent 支持多级 LLM 降级配置（FallbackLlmClient），可以在 `~/.cyber-jianghu/agent.yaml` 中修改：

```yaml
llm:
  provider: ollama
  model: qwen2.5:14b
  # 降级容灾：当主模型 403/429/超时 时，自动 fallback
  fallback_models:
    - qwen2.5:7b
    - qwen2.5:3b
```
也可以通过 Agent 的内置 Web 面板 (`http://localhost:23340/settings.html`) 直接修改并热重载。
