# Tick 引擎

## 架构概述

Tick 引擎采用**实时模式**，由两个独立组件构成：

- **TickScheduler**: 纯时钟驱动，负责周期信号和 WorldState 广播
- **IntentWorker**: 单消费者事件循环，实时处理 Agent Intent 和 TickBoundary 消息

```
TickScheduler                          IntentWorker
     │                                      │
     │  interval.tick()                      │
     │  calculate_tick_id()                  │
     │       │                              │
     │       ├──► TickBoundary ──────────►│
     │       │                              ├──► Decay + Death Check
     │       │                              ├──► Persist to DB (write-through)
     │       │                              ├──► Update DashMap
     │       │                              └──► Send ExecutionResult
     │       │                              │
     │       └──► broadcast WorldState       │
     │                                      │
     │  ┌───────────────────────────────────┘
     │  │  Agent Intent via WebSocket
     │  └───────────────────────────────────►│
     │                                          ├──► process_intent()
     │                                          │     validate → resolve → execute → mutate
     │                                          ├──► Persist to DB
     │                                          ├──► Update DashMap
     │                                          └──► Send ExecutionResult
```

**设计原则**:
- 单消费者消除所有竞态（DashMap 不存在并发写入冲突）
- write-through: persist 到 DB 确认后才更新 DashMap
- 非阻塞: handler 用 `try_send`，队列满时返回错误而非 block

## TickScheduler

纯时钟驱动，不处理任何业务逻辑。

```rust
// 每周期:
interval.tick()
calculate_tick_id()
atomic_store(accepting_tick_id, tick_id)  // 仅作为当前 tick_id 标识
send(TickBoundary { tick_id })           // 触发 IntentWorker 衰减
broadcast WorldState                     // 广播给所有 Agent
```

## IntentWorker

单消费者事件循环，顺序处理两类消息：

### WorkerMessage

```rust
pub enum WorkerMessage {
    /// Agent 提交的意图（实时处理）
    Intent { intent: Box<Intent> },
    /// Tick 周期边界信号（衰减 + 广播）
    TickBoundary { tick_id: i64 },
}
```

### 处理流程

**TickBoundary 消息**:
1. `apply_decay()` — 饥饿、口渴、物品耐久衰减
2. 环境伤害判定
3. 死亡检查（HP ≤ 0 → 推送 DeathNotification → 断开连接 → 清空背包 → 掉落物品）
4. `persist_states()` — 批量持久化到 PostgreSQL
5. `update DashMap` — write-through 成功后更新内存缓存
6. `send ExecutionResult` — 广播执行结果

**Intent 消息**:
1. `rate_limit` 检查
2. `is_alive` 检查
3. `StateProcessor::process_intent()` — 验证 → 冲突解析 → 执行 → 状态变更
4. `persist` → `update DashMap` — write-through
5. `send ExecutionResult` — 返回给 Agent

## 状态管理

- **DashMap write-through**: IntentWorker 先持久化到 PostgreSQL (await 确认)，再更新 DashMap
- **persist 失败**: DashMap 不更新，Agent 收到 `ExecutionResult(success=false)`
- **`accepting_tick_id`**: 仅作为当前 tick_id 的原子标识，**无关单机制**（实时模式持续开单）

## Tick 配置

- 周期: `real_seconds_per_tick`（可配置，默认 60 秒）
- `tick_id` = `current_unix_secs - game_epoch`（Unix 秒级时间戳）
- `accepting_tick_id` = 当前 tick_id（持续开单，无窗口关闭）
