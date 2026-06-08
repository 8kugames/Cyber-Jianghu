# 实时 Intent 处理管道

**级别**: P0 核心基石
**模块**: `crates/server`

## 1. 设计目标
提供一个零并发冲突的单消费者事件循环引擎 (`IntentWorker`)，负责串行处理所有 Agent 发起的意图（Intent）以及来自调度器的周期边界信号（TickBoundary）。

## 2. 核心机制
### 2.1 单消费者 MPSC 队列
- 所有的 WebSocket 接收端收到 Agent 的 `Intent` 后，通过 `tokio::sync::mpsc` 发送给 `IntentWorker`。
- **消除锁竞争**：无论多少个 Agent 同时移动或攻击同一个目标，处理逻辑始终是单线程串行执行，彻底避免了并发竞争和死锁。`DashMap` 在此模型下作为只读/单点写源，杜绝脏写。

### 2.2 状态 Write-Through 策略
- **内存快照回滚**：在执行 Intent 期间，先克隆 Agent 状态。如果内部逻辑验证或计算失败，通过覆盖恢复克隆的状态。
- **持久化前置 (Write-Through)**：内存状态（`AgentStateCache` / DashMap）的更新必须等待数据库 `upsert_agent_state` 返回成功后才能进行。如果持久化失败，则 DashMap 不更新，向 Agent 报错，避免产生内存与 DB 脱节的幽灵状态。

### 2.3 交互驱动即时推送 (Reactive Push)
- 动作发生后，`IntentWorker` 会立刻针对提交动作的 Agent 以及同 `node_id` 的其他 Agent 发送局部的 `WorldState` 刷新和 `ImmediateEvent`（如大喊、攻击、使用物品）。
- 这种同地广播（Co-located Broadcast）机制确保了交互的低延迟，无需等待下一个 Tick 的全局广播。

### 2.4 Subsequent Intents 管线
- Agent 单次提交可包含一个主动作与多个后续动作（Subsequent Intents），`IntentWorker` 会依次验证和执行。一旦其中某一步失败，管线立刻中断，剩余动作作废。

## 3. 架构约束
- `IntentWorker` 处理必须绝对串行化，保证同一时刻世界状态的强一致性。
- 禁止 `IntentWorker` 在执行过程中发生长时间的阻塞，持久化必须异步（`await` sqlx）。

## 4. 代码入口
- 引擎核心: `crates/server/src/tick/realtime.rs` (IntentWorker)
- 动作流转: `crates/server/src/tick/processor/processor.rs`
