# Tick 调度引擎

**级别**: P0 核心基石
**模块**: `crates/server`

## 1. 设计目标
作为游戏世界的心跳起搏器，负责推进时间、计算生理衰减以及周期性广播世界状态。这是游戏从静态数据向动态模拟转变的发动机。

## 2. 核心机制
### 2.1 纯时钟驱动 (Non-Event Driven)
- 基于 Unix 时间戳生成 `tick_id`，采用非事件驱动模式。即便没有任何 Agent 活动，Tick 依然会稳定推进。
- 在 `TickScheduler` 中，每隔配置的毫秒数（如 1000ms）触发一次状态更新。

### 2.2 生理衰减与寿终正寝
- 在每个 Tick 循环中，遍历所有存活 Agent，执行 `apply_natural_decay`。
- 定期扣除 HP（流血/中毒）、体力，增加饥饿度和口渴度。
- 检查 Agent 的 `age_ticks`，超龄则自动触发死亡（HP 置 0）。

### 2.3 状态广播 (WorldState Broadcast)
- 在完成所有衰减和环境计算后，向所有在线 Agent 的 WebSocket 连接广播当前最新的 `WorldState` 快照。
- 快照经过视距裁剪，只下发当前所在 `NodeID` 及相邻节点的信息。

### 2.4 边界事件 (TickBoundary)
- 用于处理无需高频计算的长周期任务。例如每 7 个游戏日（Tick 累计阈值）触发 `Chronicle` 传记生成，或定时触发 Vendor 自动补货。

## 3. 架构约束
- Tick 循环必须极致轻量化，严禁在主循环中执行重度 I/O 操作（如阻塞式数据库写入）或调用 LLM API。
- 保证严格的先后顺序：先处理上一周期的 Intent -> 生理衰减 -> 广播。

## 4. 代码入口
- 调度循环: `crates/server/src/tick/scheduler.rs`
- 衰减逻辑: `crates/server/src/tick/processor/decay.rs`
