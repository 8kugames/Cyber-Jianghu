# 高性能状态管理

**级别**: P0 核心基石
**模块**: `crates/server`

## 1. 设计目标
保障十万级 Agent 并发读写的内存与持久化架构，确保极高吞吐下的状态安全和低延迟广播。

## 2. 核心机制
### 2.1 DashMap 内存层
- 使用分片无锁高并发哈希表 `DashMap<Uuid, AgentState>` 作为内存缓存层。
- 承载所有的 `TickScheduler` 遍历读取请求和 `WorldState` 构建请求，使得读取性能达到纳秒级。

### 2.2 异步 Write-Through 持久化
- 使用 `sqlx` 将变更异步写入 PostgreSQL，保障数据不丢失。
- 采用 Write-Through（写穿透）策略：Intent 处理时先写入数据库（或在事务中），确认成功后再更新 DashMap 中的值，防止数据撕裂。

### 2.3 Per-agent 请求限流器
- 在 `AppState` 中维护了基于 Token Bucket 或滑动窗口的限流器。
- 防止单个恶意或失控的 Agent 在极短时间内发送海量 Intent 或 API 请求，压垮服务器或耗尽数据库连接池。

## 3. 架构约束
- 优先内存读写，数据库作为兜底的最终一致性保障和持久化源。
- 重度查询（如全局排行榜、历史记录）必须直接走数据库从副本，严禁遍历整个 DashMap 进行复杂过滤。

## 4. 代码入口
- 状态容器: `crates/server/src/state.rs`
- 缓存同步: `crates/server/src/db/agent_ops.rs`
