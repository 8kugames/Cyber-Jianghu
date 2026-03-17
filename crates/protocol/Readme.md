# Cyber-Jianghu Protocol

核心通信协议库，定义服务端与客户端之间所有共享的数据结构、消息格式和错误类型。

## 模块结构

```
crates/protocol/src/
├── lib.rs        # 模块入口和重导出
├── error.rs      # GameError 错误类型（从 common 合并）
├── messages.rs   # WebSocket 消息定义
├── types/        # 共享类型定义
│   ├── mod.rs    # 类型重导出
│   ├── actions.rs  # ActionType, Intent
│   ├── world.rs    # WorldState, WorldTime, WorldEvent
│   ├── entities.rs # AgentSelfState, Entity, InventoryItem
│   ├── locations.rs # Location, LocationNode, LocationGraph
│   ├── attributes.rs # AttributeDefinition, StatusComponent
│   ├── rules.rs    # GameRules, WorldBuildingRules
│   └── narrative.rs # NarrativeConfig, NarrativeThreshold
└── sqlx_types.rs # sqlx 数据库类型支持（feature gate）
```

## 使用方式

```toml
[dependencies]
# 基础使用
cyber-jianghu-protocol = { path = "crates/protocol" }

# 服务端需要数据库类型支持
cyber-jianghu-protocol = { path = "crates/protocol", features = ["sqlx-support"] }
```

## 消息协议

### 服务端消息 `ServerMessage`

采用 `type` 作为 tag，`snake_case` 格式：

```json
{"type": "world_state", "tick_id": 42, "agents": [...], ...}
```

| 类型 | 说明 |
|------|------|
| `registered` | 注册成功（含 `game_rules`） |
| `world_state` | 每 tick 下发的世界状态 |
| `game_rules_update` | 游戏规则热更新 |
| `world_building_rules_update` | 世界观规则热更新 |
| `dialogue` | 对话消息转发 |
| `pong` | 心跳响应 |
| `error` | 错误消息 |

### 客户端消息 `ClientMessage`

```json
{"type": "intent", "tick_id": 42, "action_type": "idle", ...}
```

| 类型 | 说明 |
|------|------|
| `intent` | 提交意图（扁平化字段） |
| `dialogue` | 对话消息 |

### 对话消息 `DialogueMessage`

采用 `message_type` 作为 tag：

| 类型 | 说明 |
|------|------|
| `request` | 请求对话 |
| `accept` | 接受对话 |
| `reject` | 拒绝对话 |
| `content` | 对话内容 |
| `end` | 结束对话 |

## 核心类型

### 动作与意图

```rust
use cyber_jianghu_protocol::{ActionType, Intent};

// ActionType 是枚举，支持数据驱动扩展
let action = ActionType::Idle;
let action = ActionType::Move { target: "location_id".to_string() };

// Intent 包含完整的意图信息
let intent = Intent {
    agent_id,
    tick_id,
    action_type: ActionType::Idle,
    action_data: None,
    priority: 0,
    thought_log: None,
};
```

### 世界状态

```rust
use cyber_jianghu_protocol::{WorldState, WorldTime, WorldEvent};

// WorldState 是每 tick 下发的完整世界快照
struct WorldState {
    pub tick_id: i64,
    pub world_time: WorldTime,
    pub agents: Vec<AgentSelfState>,
    pub entities: Vec<Entity>,
    pub scene_items: Vec<SceneItem>,
    pub events: Vec<WorldEvent>,
    // ...
}
```

### 实体与状态

```rust
use cyber_jianghu_protocol::{AgentSelfState, Entity, InventoryItem, SceneItem};

// AgentSelfState - Agent 的自身状态视图
struct AgentSelfState {
    pub agent_id: Uuid,
    pub name: String,
    pub location: String,
    pub status: StatusComponent,  // 属性组件
    pub inventory: Vec<InventoryItem>,
    // ...
}

// Entity - 场景中的实体
// SceneItem - 场景中的物品
```

### 地点与地图

```rust
use cyber_jianghu_protocol::{Location, LocationNode, LocationGraph};

// LocationGraph - 地点图（节点 + 边）
// LocationNode - 地点节点
// Location - 简化的地点信息
```

### 游戏规则

```rust
use cyber_jianghu_protocol::{GameRules, WorldBuildingRules, AvailableAction};

// GameRules - 核心游戏规则
// WorldBuildingRules - 世界观规则
// AvailableAction - 可用动作定义
```

## 错误类型

`GameError` 定义了所有游戏相关的错误类型：

```rust
use cyber_jianghu_protocol::GameError;

match result {
    Err(GameError::AgentDead { agent_id }) => { /* 处理 */ }
    Err(GameError::ItemNotFound(item_id)) => { /* 处理 */ }
    Err(GameError::InvalidAction { reason }) => { /* 处理 */ }
    // ...
}
```

主要错误类型：

| 类别 | 错误 |
|------|------|
| Agent | `AgentDead`, `AgentNotFound` |
| 动作 | `InvalidAction`, `ActionCooldown`, `InsufficientResources` |
| 物品 | `ItemNotFound`, `ItemNotUsable`, `InventoryFull` |
| 对话 | `DialogueNotFound`, `DialogueRejected` |
| 技能 | `SkillNotFound`, `SkillNotReady` |
| 客户端 | `NotAuthenticated`, `InvalidToken` |

## 典型用法

### 序列化与反序列化

```rust
use cyber_jianghu_protocol::{ClientMessage, Intent, ServerMessage};

// Intent -> ClientMessage -> JSON
let intent = Intent::idle(agent_id, tick_id);
let msg = ClientMessage::from_intent(intent);
let json = serde_json::to_string(&msg)?;

// JSON -> ServerMessage
let server_msg: ServerMessage = serde_json::from_str(&json_str)?;
```

### 处理服务端消息

```rust
use cyber_jianghu_protocol::ServerMessage;

match server_msg {
    ServerMessage::WorldState { tick_id, agents, .. } => {
        // 处理世界状态
    }
    ServerMessage::Registered { agent_id, game_rules, .. } => {
        // 注册成功
    }
    ServerMessage::Dialogue { session, .. } => {
        // 对话消息
    }
    _ => {}
}
```

## Feature Flags

| Feature | 说明 |
|---------|------|
| `sqlx-support` | 启用 sqlx 数据库类型支持（仅服务端需要） |

## 版本

`PROTOCOL_VERSION` 与 crate 版本保持一致：

```rust
pub const PROTOCOL_VERSION: &str = env!("CARGO_PKG_VERSION");
```

## 相关文档

- [CLAUDE.md](../../CLAUDE.md) - 项目开发指南
- [Agent](../agent/README.md) - Agent SDK 开发指南
- [Server](../server/README.md) - 服务端开发指南
