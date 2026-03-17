# Cyber-Jianghu Server

游戏服务端（"天道"），负责世界状态维护、Tick 结算、Agent 注册与 WebSocket 通信。

## 架构概览

```
crates/server/src/
├── main.rs            # 入口点（初始化 + 启动）
├── config.rs          # 配置加载
├── state.rs           # AppState 共享状态
├── paths.rs           # 路径配置
├── actions/           # 动作执行系统
│   ├── mod.rs
│   ├── types.rs
│   ├── executor/      # 动作执行器（basic, combat, interaction）
│   └── validator.rs   # 动作验证
├── tick/              # Tick 引擎
│   ├── mod.rs         # TickScheduler
│   ├── persistence.rs # 状态持久化
│   ├── broadcaster.rs # 状态广播
│   └── event_manager.rs
├── websocket/         # WebSocket 管理
│   ├── mod.rs
│   ├── broadcast.rs
│   └── types.rs
├── handlers/          # HTTP 路由处理
│   ├── system.rs      # 健康检查
│   ├── agent.rs       # Agent 注册
│   ├── context.rs     # 叙事上下文
│   ├── validation.rs  # 动作验证
│   ├── dashboard.rs   # Dashboard API
│   ├── config_editor.rs
│   └── auth.rs        # Token 认证
├── db/                # 数据库访问
│   ├── mod.rs
│   ├── agent_ops.rs
│   ├── state_ops.rs
│   └── ground_item_ops.rs
├── game_data/         # 游戏数据系统
│   ├── mod.rs
│   ├── loader.rs
│   ├── cache.rs
│   ├── loaders/       # JSON 加载器
│   ├── registry/      # 统一注册表
│   ├── formula_engine/ # 公式引擎
│   └── types/         # 数据类型
├── models/            # 数据模型
│   ├── agent.rs
│   ├── state_impl.rs
│   ├── items.rs
│   └── ...
├── items/             # 物品系统
│   ├── mod.rs
│   ├── types.rs
│   ├── registry.rs
│   ├── system.rs      # 效果应用
│   └── tests.rs
├── inventory/         # 背包系统
│   ├── mod.rs
│   ├── manager.rs
│   └── types.rs
└── dialogue/          # 对话系统
    ├── mod.rs
    ├── session.rs
    └── types.rs
```

## 核心概念

### Tick 引擎

Tick 引擎是游戏的心脏，驱动世界演化：

```
┌─────────────────────────────────────────────┐
│                 Tick 循环                    │
│  1. 收集 Intent（从 IntentManager）          │
│  2. 验证 Intent（动作合法性）                 │
│  3. 执行 Intent（更新世界状态）               │
│  4. 持久化状态（写入数据库）                  │
│  5. 广播 WorldState（推送给所有 Agent）       │
└─────────────────────────────────────────────┘
```

### 动作系统

数据驱动的动作执行：

```rust
// 动作定义从 config/actions.json 加载
// 执行器按 ActionType 分发
match action_type {
    ActionType::Idle => executor::basic::execute_idle(...),
    ActionType::Move { target } => executor::basic::execute_move(...),
    ActionType::Attack { target_id } => executor::combat::execute_attack(...),
    ActionType::Dialogue { target_id } => executor::interaction::execute_dialogue(...),
    // ...
}
```

### 游戏数据系统

所有游戏机制通过 JSON 配置驱动：

| 配置文件 | 说明 |
|---------|------|
| `actions.json` | 动作定义、参数、验证规则 |
| `attributes.json` | 属性定义（主属性、状态、派生） |
| `items.json` | 物品定义、效果、堆叠 |
| `locations.json` | 地点图（节点 + 边） |
| `recipes.json` | 合成配方 |
| `game_rules.json` | 核心规则 |
| `narrative_config.json` | 叙事化阈值配置 |

## 运行依赖

- Rust 1.75+
- PostgreSQL 14+
- Docker（可选）

## 配置

### 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `DATABASE_URL` | PostgreSQL 连接字符串 | 必需 |
| `SERVER_HOST` | 监听地址 | `0.0.0.0` |
| `SERVER_PORT` | 监听端口 | `23333` |
| `TICK_DURATION_SECS` | Tick 周期（秒） | `60` |
| `ADMIN_READ_TOKEN` | Dashboard 只读 Token | 自动生成 |
| `ADMIN_WRITE_TOKEN` | Dashboard 读写 Token | 自动生成 |

### 配置文件

- `.env` - 环境变量
- `config/*.json` - 游戏数据配置

## HTTP API

### 公开接口

| 端点 | 方法 | 说明 |
|------|------|------|
| `/` | GET | 欢迎信息 |
| `/health` | GET | 健康检查 |
| `/api/v1/agent/register` | POST | Agent 注册 |
| `/api/v1/agent/{id}/context` | GET | 叙事上下文 |
| `/api/v1/validate-action` | POST | 动作验证 |

### Dashboard API（需 Token）

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/dashboard/stats` | GET | 统计信息 |
| `/api/dashboard/agents` | GET | 在线 Agent |
| `/api/dashboard/agents/offline` | GET | 离线 Agent |
| `/api/dashboard/agent/{id}` | GET | Agent 详情 |

### 配置编辑 API（需 Token）

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/config` | GET | 列出配置文件 |
| `/api/config/{filename}` | GET | 读取配置 |
| `/api/config/{filename}` | PUT | 更新配置 |

## WebSocket API

- 连接：`ws://{host}:{port}/ws?token={auth_token}`
- 协议：`cyber-jianghu-protocol`（ServerMessage / ClientMessage）

### 服务端下发

| 消息 | 说明 |
|------|------|
| `registered` | 注册成功（含 game_rules） |
| `world_state` | 每 tick 世界快照 |
| `game_rules_update` | 规则热更新 |
| `dialogue` | 对话消息 |
| `pong` | 心跳响应 |

### 客户端上报

| 消息 | 说明 |
|------|------|
| `intent` | 提交意图 |
| `dialogue` | 对话消息 |

## 开发指南

### 启动开发服务器

```bash
# 使用 Docker 启动完整环境
./scripts/cyber-jianghu.sh start

# 或手动启动
cargo run -p cyber-jianghu-server
```

### 添加新动作

1. 在 `config/actions.json` 添加动作定义
2. 在 `actions/executor/` 添加执行器实现
3. 在 `actions/executor/mod.rs` 注册分发

### 添加新配置类型

1. 在 `game_data/types/` 定义类型
2. 在 `game_data/loaders/` 添加加载器
3. 在 `game_data/registry/` 添加注册表

### 测试

```bash
# 运行所有测试
cargo test -p cyber-jianghu-server

# 运行特定测试
cargo test -p cyber-jianghu-server test_name
```

### 数据库迁移

迁移文件位于 `crates/server/migrations/`，服务启动时自动执行。

## 静态文件

管理后台静态文件从 `static/` 目录提供：
- 访问：`http://localhost:23333/admin`
- 需要 Token 认证

## 依赖关系

```
server
  ├── protocol (共享类型、GameError)
  └── 外部依赖 (axum, tokio, sqlx, serde)
```

## 相关文档

- [CLAUDE.md](../../CLAUDE.md) - 项目开发指南
- [Protocol](../protocol/README.md) - 通信协议定义
- [Agent](../agent/README.md) - Agent SDK 开发指南
