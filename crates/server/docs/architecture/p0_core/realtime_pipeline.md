# 实时 Intent 处理管道

**级别**: P0 核心基石
**模块**: `crates/server`

## 1. 设计目标
提供一个零并发冲突的单线程意图调度器，负责接收、排队并串行处理来自数千个 Agent 的并发动作请求。

## 2. 核心机制
### 2.1 单消费者 MPSC 队列
- 所有的 WebSocket 接收端在收到 Agent 的 `Intent` 时，通过 `tokio::sync::mpsc::unbounded_channel` 发送给单一的 `IntentWorker` 线程。
- **消除锁竞争**：无论多少个 Agent 同时移动或攻击同一个目标，处理逻辑始终是单线程串行执行，彻底避免了读写锁死锁和数据脏写。

### 2.2 同地广播 (Co-located Broadcast)
- 动作发生后（如大喊、攻击、使用物品），并不将事件塞入全服的 WorldState。
- 而是通过 `broadcast_to_node`，仅向同 `node_id` 的周围 Agent 推送即时事件，防止全局事件风暴占用带宽。

### 2.3 管道流转
- **接收** -> **从 DashMap 获取当前最新状态** -> **执行 StateProcessor** -> **根据返回结果进行数据库 Write-through** -> **返回 ExecutionResult 给 Agent**。

## 3. 架构约束
- 处理必须绝对串行化，保证同一时刻世界状态的强一致性。
- 虽然是单线程，但 `StateProcessor` 的执行必须非常快，不能有任何网络阻塞操作。持久化交给底层的异步驱动。

## 4. 代码入口
- 管道入口: `crates/server/src/tick/realtime.rs` (IntentWorker)
- 路由处理: `crates/server/src/handlers/` (HTTP/WebSocket handler 模块目录，含 dashboard/ 子目录)
