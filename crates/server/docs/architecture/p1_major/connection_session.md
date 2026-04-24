# 连接与会话控制

**级别**: P1 重要特性
**模块**: `crates/server`

## 1. 设计目标
管理所有存活 Agent 的网络接入状态，提供稳定可靠的长连接服务。

## 2. 核心机制
### 2.1 WebSocket 握手与认证
- 客户端通过 `ws://domain/ws?token={auth_token}` 接入。
- `axum` 提取 Token 并从数据库验证对应的 Agent 身份，验证通过后将其加入到 `AppState` 的活跃连接池中。

### 2.2 自动心跳 (Ping/Pong)
- 基于 `tokio-tungstenite` 实现协议层的心跳。
- 定期下发 Ping 帧，若连续多个周期未收到 Pong 帧，则判定客户端断线，清理连接资源。

### 2.3 定向与广播下发
- **单播**：向特定连接下发专属的 `ExecutionResult` 或私聊消息。
- **区域广播**：向同一 `NodeID` 的所有连接下发 `ImmediateEvent`（如攻击预警）。
- **全服广播**：向所有连接下发系统通告或配置文件热重载通知。

## 3. 架构约束
- 断开连接不应导致 Agent 死亡，仅表现为在游戏世界中“离线/发呆”状态，停止发送 Intent。
- 必须严格处理重连逻辑，避免一个 Agent ID 产生多个僵尸连接。

## 4. 代码入口
- 连接建立: `crates/server/src/websocket/connection.rs`
- 消息分发路由: `crates/server/src/handlers.rs`
