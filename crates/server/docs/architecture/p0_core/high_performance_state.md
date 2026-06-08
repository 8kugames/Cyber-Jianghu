# 高性能状态管理

**级别**: P0 核心基石
**模块**: `crates/server`

## 1. 设计目标
保障大规模 Agent 并发读写的内存与持久化架构，确保极高吞吐下的状态安全、低延迟广播与崩溃恢复能力。

## 2. 核心机制
### 2.1 DashMap 内存层 (`AgentStateCache`)
- 使用分片无锁高并发哈希表 `DashMap<Uuid, AgentState>` 作为全局唯一的内存缓存层。
- 承载所有的 `TickScheduler` 遍历读取请求和 `WorldState` 构建请求。因为所有的写操作都被局限在 `IntentWorker` 的单一消费者线程中，所以 `DashMap` 不会发生并发写入冲突。

### 2.2 Write-Through (写穿透) 持久化
- 使用 `sqlx` 将变更异步写入 PostgreSQL，保障数据不丢失。
- 采用 Write-Through 策略：`IntentWorker` 在处理完 Intent 后，先将更新的 `AgentState` 通过 `upsert_agent_state` 写入数据库，**确认成功后**，再将其插入 `DashMap` 中更新内存状态。
- 如果持久化失败，`DashMap` 将保留原值，并给 Agent 发送执行失败的通知。这彻底消除了内存状态与数据库脱节的幽灵状态。

### 2.3 Per-agent 请求限流器 (`RateLimiter`)
- 在共享的 `AppState` 中维护了一个 `RateLimiter`。
- 记录每个 Agent 的最后活动时间，防止单个恶意或失控的 Agent 在极短时间内发送海量 Intent，占用 `IntentWorker` 队列或耗尽数据库连接池。

## 3. 架构约束
- 优先内存读写，数据库作为兜底的最终一致性保障和持久化源。
- 严禁越过 `IntentWorker` 直接对 `DashMap` 进行并发写入，否则将破坏 Write-Through 的状态一致性。

## 4. 代码入口
- 状态容器与限流器: `crates/server/src/state.rs`
- 持久化操作: `crates/server/src/db/agent_ops.rs`
- 持久化时机调度: `crates/server/src/tick/realtime.rs`
