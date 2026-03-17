# Cyber-Jianghu Agent SDK

Agent SDK 用于连接 Cyber-Jianghu 游戏服务器，提供多种决策模式和 AI 增强模块。

## 架构概览

```
crates/agent/src/
├── transport/      # 通信层（WebSocket 客户端）
├── core/           # 核心逻辑（Agent 组装、生命周期、工具）
├── runtime/        # 运行模式（决策函数、HTTP API）
├── ai/             # 智能增强模块
│   ├── llm/        # LLM 客户端（OpenClaw、直连）
│   ├── cognitive/  # 认知引擎 + 叙事化
│   ├── memory/     # 记忆系统（工作、情景、语义、归档）
│   ├── persona/    # 动态人设系统
│   ├── validator/  # 意图验证（规则引擎 + LLM）
│   ├── dialogue/   # 对话系统
│   ├── relationship/ # 关系管理
│   ├── lifespan/   # 寿命计算
│   └── prompts.rs  # Prompt 模板
├── config.rs       # 配置
├── models.rs       # 数据模型
└── bin/            # CLI 入口
```

## 数据流

```
Server ─[WebSocket]→ Transport ─[WorldState]→ Runtime ─[Intent]→ Transport ─[WebSocket]→ Server
```

## 运行模式

| 模式 | 说明 | 适用场景 |
|------|------|----------|
| `http` | HTTP API 服务器 | OpenClaw 集成、外部 AI 服务 |
| `cognitive` | 内置认知引擎 | 独立运行、测试 |
| `simple` | 简单规则决策 | 基于生理需求的快速原型 |
| `idle` | 只空闲 | 调试、占位 |
| `stdio` | 标准输入输出 | 外部程序决策 |
| `tcp` | TCP 服务器 | 外部程序决策 |

## 快速开始

### 安装

```bash
cargo install --path crates/agent
```

### HTTP 模式（推荐用于 OpenClaw）

```bash
cyber-jianghu-agent run --mode http --port 23340
```

启动后可通过 HTTP API 与 Agent 交互：

```bash
# 获取当前世界状态
curl http://localhost:23340/api/v1/state

# 获取叙事上下文（Markdown 格式，推荐用于 LLM）
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

### Cognitive 模式（内置 AI）

```bash
cyber-jianghu-agent run --mode cognitive
```

## HTTP API 参考

### 核心 API

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1` | GET | API 发现（返回所有可用 API） |
| `/api/v1/health` | GET | 健康检查 |
| `/api/v1/state` | GET | 获取当前 WorldState |
| `/api/v1/context` | GET | 获取叙事上下文（Markdown） |
| `/api/v1/attributes` | GET | 获取属性值（禁止存储到记忆） |
| `/api/v1/intent` | POST | 提交意图到服务器 |
| `/api/v1/validate` | POST | 提交前验证动作 |

### 记忆 API

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1/memory/recent` | GET | 获取最近记忆 |
| `/api/v1/memory/search` | POST | 搜索记忆 |
| `/api/v1/memory` | POST | 存储记忆 |

### 关系 API

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1/relationship/list` | GET | 获取所有关系 |
| `/api/v1/relationship/:id` | GET | 获取特定关系 |
| `/api/v1/relationship` | POST | 更新关系 |

### 寿命 API

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/v1/lifespan` | GET | 获取寿命状态 |

## 核心模块

### transport（通信层）

WebSocket 客户端实现，负责与游戏服务器通信：

```rust
use cyber_jianghu_agent::WebSocketClient;

let client = WebSocketClient::new(server_config);
client.connect().await?;
let state = client.receive_state().await?;
client.send_intent(intent).await?;
```

### core（核心逻辑）

- `Agent` - Agent 实体
- `AgentBuilder` - Builder 模式组装 Agent
- `lifecycle` - 生命周期管理
- `tools` - 工具定义

```rust
use cyber_jianghu_agent::{Agent, AgentBuilder};

let agent = AgentBuilder::new()
    .config(config)
    .decision_callback(http_decision)
    .build()?;

agent.run().await?;
```

### runtime（运行模式）

提供各种决策函数：

```rust
use cyber_jianghu_agent::{http_decision, cognitive_decision};

// HTTP 模式
let intent = http_decision(&state, &http_config).await?;

// Cognitive 模式
let intent = cognitive_decision(&state, &llm_client, &cognitive_config).await?;
```

## AI 模块

### LLM 客户端（ai/llm）

支持多种 LLM 后端：

- `LlmClient` - 统一接口
- `OpenClawClient` - OpenClaw 协议
- `DirectClient` - 直连 OpenAI 兼容 API

```rust
use cyber_jianghu_agent::LlmClient;

let client = LlmClient::new_openclaw(endpoint);
let response = client.complete(prompt).await?;
```

### 认知引擎（ai/cognitive）

多阶段认知管道：

```
感知 → 动机 → 规划 → 决策
```

- `NarrativeEngine` - 属性叙事化描述
- `output_schema` - 结构化输出定义

### 记忆系统（ai/memory）

四层记忆架构：

| 层级 | 说明 | 后端 |
|------|------|------|
| Working | 短期工作记忆 | 内存 |
| Episodic | 情景记忆 | SQLite |
| Semantic | 语义记忆 | SQLite + 向量 |
| Archive | 归档记忆 | SQLite |

```rust
use cyber_jianghu_agent::{MemoryManager, MemoryManagerConfig};

let memory = MemoryManager::new(config)?;
memory.store("event description", importance).await?;
let results = memory.search("query").await?;
```

### 人设系统（ai/persona）

动态人设管理：

- `DynamicPersona` - 动态人设状态
- `EventTraitMapper` - 事件到特质映射
- `TraitType` - 特质类型定义

```rust
use cyber_jianghu_agent::{DynamicPersona, EventTraitMapper};

let persona = DynamicPersona::new(initial_traits);
persona.apply_event(&event, &mapper);
```

### 意图验证（ai/validator）

两级验证系统：

1. **规则引擎** - 快速硬规则检查
2. **LLM 验证** - 深度语义验证

```rust
use cyber_jianghu_agent::{IntentValidator, ValidationRequest};

let validator = IntentValidator::new(llm_client, config);
let result = validator.validate(request).await?;

match result {
    ValidationResult::Approved => { /* 执行 */ }
    ValidationResult::Rejected { rejection_type, .. } => { /* 处理拒绝 */ }
}
```

### 关系管理（ai/relationship）

Agent 间关系追踪：

- `RelationshipStore` - 关系存储
- `KeyEvent` - 关键事件记录
- 叙事化描述生成

### 寿命计算（ai/lifespan）

基于年龄的效果计算：

```rust
use cyber_jianghu_agent::{LifespanCalculator, LifespanConfig};

let calculator = LifespanCalculator::new(config);
let status = calculator.calculate(age);
```

## 配置

配置文件位于 `~/.cyber-jianghu/config/`：

- `config.json` - 主配置
- `narrative_config.json` - 叙事配置（从服务器获取）

## 开发指南

### 添加新的决策模式

1. 在 `runtime/decision/` 创建新模块
2. 实现决策函数签名：`async fn(WorldState) -> Intent`
3. 在 `runtime/mod.rs` 注册

### 添加新的 AI 模块

1. 在 `ai/` 创建新目录
2. 实现 `ai/mod.rs` 中导出
3. 更新 `lib.rs` 重导出

### 测试

```bash
cargo test -p cyber-jianghu-agent
```

## 依赖关系

```
agent
  ├── protocol (共享类型、GameError)
  └── 外部依赖 (tokio, serde, sqlx, reqwest)
```

## 相关文档

- [CLAUDE.md](../../CLAUDE.md) - 项目开发指南
- [Protocol](../protocol/README.md) - 通信协议定义
- [Server](../server/README.md) - 服务端开发指南
