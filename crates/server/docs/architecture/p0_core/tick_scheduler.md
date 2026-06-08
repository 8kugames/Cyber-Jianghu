# Tick 调度引擎 (TickScheduler)

**级别**: P0 核心基石
**模块**: `crates/server`

## 1. 设计目标
作为游戏世界的心跳起搏器，负责推进时间、周期性触发衰减（经由 IntentWorker）并周期性向客户端广播客观世界状态。

## 2. 核心机制
### 2.1 纯时钟驱动 (Pure Clock Driven)
- 基于 Unix 时间戳与配置的“游戏纪元”计算出当前的绝对秒数作为 `tick_id`。即便没有 Agent 活动，时间流逝（Tick）依然稳定推进。
- 在 `TickScheduler` 主循环中，每隔配置文件设定的真实秒数（`real_seconds_per_tick`）触发一次处理。

### 2.2 Tick 边界信号 (TickBoundary)
- `TickScheduler` 本身不直接计算生理衰减和死亡，而是通过 MPSC 通道向 `IntentWorker` 发送 `TickBoundary` 消息。
- `IntentWorker` 收到后，执行生理值（HP、饱食度、水分）的自然衰减计算，检测 Agent 是否存活，并执行批量的数据库写入。
- 此设计确保了所有的状态变动（包括自然衰减与 Agent 动作）都在 `IntentWorker` 这个单消费者线程中安全地串行处理。

### 2.3 状态广播 (WorldState Broadcast)
- `TickScheduler` 在发送边界信号后，从 `AgentStateCache` (DashMap) 中抓取最新的全部 Agent 状态，构建视距裁剪后的 `WorldState`。
- 向所有连接在线的 Agent 的 WebSocket 连接广播最新的环境感知信息。

### 2.4 热重载与长周期任务
- **热重载检查**：在每个 Tick 中，检查 YAML 配置（如 `actions.yaml`, `game_rules.yaml` 等）的文件修改时间。如果检测到变更，立刻重载配置并通过 WebSocket 的 `ConfigUpdate` 推送给所有在线 Agent。
- **游戏日统计**：每逢一个游戏日结束，生成并推送 Daily Summaries 统计。
- **群像传记生成**：每隔配置的周期（默认 7 游戏日），触发 Chronicle（传记）的异步生成和入库。

## 3. 架构约束
- Tick 循环必须轻量化，绝不直接执行重度数据库读写（除查询基础映射）或 LLM 请求。
- 采用非阻塞的发送机制：向 `IntentWorker` 发送边界信号时不阻塞自身广播循环。

## 4. 代码入口
- 调度主循环: `crates/server/src/tick/scheduler.rs`
- 衰减逻辑实现: `crates/server/src/tick/decay.rs` (由 IntentWorker 调用)
