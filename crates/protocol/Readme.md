# Cyber-Jianghu Protocol

核心通信协议库，定义服务端与客户端之间所有共享的数据结构、消息格式和错误类型。本协议层采用无状态、数据驱动的设计理念，为游戏引擎和 AI Agent 提供统一的类型边界。

## 模块结构

```
crates/protocol/src/
├── lib.rs        # 模块入口和重导出
├── error.rs      # GameError 错误类型（统一的错误处理）
├── messages.rs   # WebSocket 消息定义（ServerMessage, ClientMessage, DialogueMessage）
├── types/        # 游戏世界核心共享类型定义
│   ├── mod.rs    # 类型重导出
│   ├── actions.rs  # 动作与意图定义 (ActionType, Intent)
│   ├── world.rs    # 世界状态 (WorldState, WorldTime, WorldEvent)
│   ├── entities.rs # 实体类型 (AgentSelfState, Entity, InventoryItem)
│   ├── locations.rs # 地理位置系统 (Location, LocationNode, LocationGraph)
│   ├── attributes.rs # 属性系统 (AttributeDefinition, StatusComponent)
│   ├── rules.rs    # 游戏规则 (GameRules, WorldBuildingRules)
│   └── narrative.rs # 叙事配置 (NarrativeConfig, NarrativeThreshold)
└── sqlx_types.rs # sqlx 数据库类型支持（通过 features 控制）
```

## 接口说明 (For Developers)

### 1. 消息协议 (messages.rs)

所有的前后端交互均基于 WebSocket 进行，定义在 `ServerMessage` 和 `ClientMessage` 枚举中。

#### ServerMessage (服务端 -> 客户端)
下发消息通过 `type` 字段进行标签化区分：
- `Registered(RegisteredData)`: 注册成功响应，包含初始 `game_rules` 和身份 Token。
- `WorldState(WorldState)`: 每个 Tick 广播的世界状态快照。
- `GameRulesUpdate(GameRules)`: 核心规则的热更新通知。
- `WorldBuildingRulesUpdate(WorldBuildingRules)`: 世界观设定的热更新通知。
- `Dialogue(DialogueMessage)`: 对话系统转发消息。
- `Pong(i64)`: 客户端 Ping 的响应。
- `Error(String)`: 业务或系统错误信息。

#### ClientMessage (客户端 -> 服务端)
- `Intent(Intent)`: 提交当前 Tick 的决策意图。
- `Dialogue(DialogueMessage)`: 发起或回复对话。
- `Ping(i64)`: 保持心跳。

#### DialogueMessage (对话系统)
- `Request(DialogueRequest)`: 请求与目标发起对话。
- `Accept(DialogueAccept)`: 接受对方的对话请求。
- `Reject(DialogueReject)`: 拒绝对方的对话请求。
- `Content(DialogueContent)`: 实际对话内容载荷。
- `End(DialogueEnd)`: 主动终止对话。

### 2. 核心领域模型 (types/)

#### 意图与动作 (actions.rs)
- `Intent`: Agent 的决策载体。包含 `agent_id`, `tick_id`, `action_type`, 优先级 `priority` 以及供后续分析的 `thought_log`。
- `ActionType`: 枚举定义所有支持的动作（如 `Idle`, `Move`, `Attack`, `Dialogue`, `Gather` 等）。这是数据驱动执行引擎的入口。

#### 世界状态 (world.rs)
- `WorldState`: 包含当前 Tick ID、游戏时间 (`WorldTime`)、存活的 Agent 列表、地上的物品 (`GroundItem`)，以及最近发生的公开事件 (`WorldEvent`)。

#### 实体状态 (entities.rs)
- `AgentSelfState`: Agent 自身视角的详细状态，包含属性 (`attributes`)、背包 (`inventory`)、健康状态等，仅对自身可见。
- `Entity`: 通用的地图实体描述。

#### 叙事配置 (narrative.rs)
- `NarrativeConfig` / `NarrativeThreshold`: 提供了将数值状态（如健康度 30%）映射为自然语言描述（如“受了重伤，血流不止”）的配置结构，专为 LLM 认知设计。

## 使用方式

在 Cargo.toml 中引入：

```toml
[dependencies]
# 基础使用（Agent 端）
cyber-jianghu-protocol = { path = "crates/protocol" }

# 服务端需要数据库持久化支持
cyber-jianghu-protocol = { path = "crates/protocol", features = ["sqlx-support"] }
```

## 扩展建议
- 新增动作类型时，需在 `ActionType` 枚举中添加新变体，并在 `crates/server/config/actions.yaml` 中增加对应的参数和校验配置。
- 所有的 `struct` 都应实现 `Serialize, Deserialize, Debug, Clone`，以支持跨进程传输和缓存。
