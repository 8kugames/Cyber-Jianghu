# 连接与会话控制

**级别**: P1 重要特性
**模块**: `crates/server`

## 1. 设计目标
管理所有存活 Agent 的网络接入状态，提供稳定可靠的 WebSocket 双向长连接，并实现严格的设备身份绑定与防刷保护。

## 2. 核心机制
### 2.1 WebSocket 握手与设备认证
- 客户端通过 `ws://host/ws?token={auth_token}` 接入。
- `axum` 在建立连接时校验 Token，并通过查询数据库确保设备注册的合法性。
- 通过 `ConnectionManager` 和 `AgentToDeviceMap` 在内存中维护映射，确保连接的定向下发。

### 2.2 自动心跳与生命周期管理
- 依赖底层的 WebSocket 框架（`tokio-tungstenite` 或 `axum` 内置）维护连接活性。
- 如果客户端异常断线，连接会被从 `ConnectionManager` 中移除。断线不导致 Agent 在游戏世界死亡，仅使其停止提交 Intent（变为发呆状态）。

### 2.3 定向与广播下发
- **单播**：向特定 Agent 下发专属的 `ExecutionResult` 或验证失败的报错信息。
- **即时响应 (Reactive Push)**：Agent 执行动作后，Server 立刻向提交动作的 Agent 及同节点的在线 Agent 推送最新的 `WorldState` 与 `ImmediateEvent`。
- **全服广播**：在 Tick 边界向所有在线 Agent 广播周期性的 `WorldState`，或者在热重载时广播 `ConfigUpdate`（如游戏规则或 Prompt 模板更新）。

### 2.4 限流防刷保护 (Rate Limiting)
- 在连接层接入 `RateLimiter`（存在于 `AppState`），以控制单个 Agent 发送 Intent 的频率。
- 如果超过配置频率（`rate_limit_ms`），请求会被服务器静默丢弃或拒绝。后台运行清理任务移除过期的速率限制记录以防内存泄漏。

## 3. 架构约束
- 必须严格处理重连逻辑。新连接建立时必须剔除旧连接，避免一个 Agent 产生多个僵尸连接接收广播。
- 发送逻辑必须是非阻塞的（使用 `try_send` 或类似的机制），当客户端接收缓慢导致队列满时，服务端应当丢弃消息而非阻塞主线程。

## 4. 代码入口
- 核心连接处理: `crates/server/src/websocket/connection.rs` 和 `handler.rs`
- 速率限制: `crates/server/src/state.rs`
