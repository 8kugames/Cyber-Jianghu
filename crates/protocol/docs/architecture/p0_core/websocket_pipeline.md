# WebSocket 全双工通信管线

**级别**: P0 核心基石
**模块**: `crates/protocol`

## 1. 设计目标
作为 Server 与 Agent 之间的实时数据交互管道，负责下发世界状态（WorldState）和上报 Agent 意图（Intent）。这是游戏“身心分离”架构的物理连接线。取代传统的 HTTP 短轮询，实现低延迟的双向数据同步。

## 2. 核心机制
### 2.1 全双工通信
基于 `tokio-tungstenite` 建立长连接，允许 Server 主动向 Agent 广播状态变更，同时允许 Agent 异步提交决策。

### 2.2 消息协议结构
采用 JSON 格式进行序列化，定义了 `ServerMessage` 和 `ClientMessage` 两个核心枚举。
- **ServerMessage (Server -> Agent)**
  - `WorldState`: 周期性下发的世界快照，包含周围节点、实体、物品等。
  - `Registered`: 建立连接后下发的初始凭证和基本规则。
  - `ExecutionResult`: 对 Agent 提交 Intent 的执行结果（成功或失败及错误原因）。
  - `AgentDied`: 死亡通知，强制 Agent 离线或转入观战模式。
  - `ImmediateEvent`: 绕过 Tick 周期的高优先级事件（如对话、攻击预警）。
  - `GameRulesUpdate`: 热重载配置下发。

- **ClientMessage (Agent -> Server)**
  - `Intent`: 包含行动类型（如 `attack`, `move`）、目标及参数。
  - `Dialogue`: 用于 NPC 之间或与玩家的实时对话交互序列。

### 2.3 心跳保活与重连
- 客户端定时发送 `Ping` 帧，服务端响应 `Pong`。
- Server 在 `connection_session` 模块维护在线连接池，长时间未收到 Ping 则主动断开。
- Agent 在网络波动时具有指数退避（Exponential Backoff）重连能力。

## 3. 架构约束
- 必须保证通信的低延迟与高吞吐。
- `POST /api/v1/intent` 接口被禁用，所有意图提交强制走 WebSocket，以确保处理的顺序和一致性。
- 序列化必须对反序列化错误宽容，未知字段不应导致连接崩溃。

## 4. 代码入口
- 协议定义: `crates/protocol/src/messages.rs`
- 客户端实现: `crates/agent/src/infra/transport/websocket.rs`
- 服务端实现: `crates/server/src/websocket/connection.rs`
