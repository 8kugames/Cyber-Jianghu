# 异步即时事件引擎 (SessionTriageEngine)

**级别**: P2 体验增强
**模块**: `crates/agent`

## 1. 第一性原理与设计目标
游戏世界是连续且嘈杂的，而 Agent 的认知推理是离散的（Tick 为单位）。如果将所有事件（风吹草动、远处的战斗、直接针对自己的对话）都立即塞入 Agent 的主推理循环，将导致极高的 Token 消耗和频繁的决策打断。
`SessionTriageEngine` 的设计目标是作为**事件的“分诊台”与记忆的“降维压缩器”**，在不阻塞主决策循环的前提下，异步地对大量噪声进行过滤和压缩。

## 2. 核心机制

### 2.1 异步分诊循环 (Triage Batching)
- `SessionTriageEngine` 作为独立的 tokio 任务后台运行。
- 监听 `EventStore` 发出的 `Notify` 信号，触发防抖（Debounce）收集窗口。
- 收集到一定数量的 Pending 事件后，将其打包发送给 LLM（或者超时时回退到规则引擎 `fallback_priority_split`），要求模型将其分类为 `urgent`（紧急，立刻关注）、`batch`（一般，稍后处理）或 `ignore`（忽略，噪音）。
- 处理结果写回数据库，标记 `triage_status`。

### 2.2 每日摘要生成 (Daily Summary)
- 在游戏日结束时，`check_game_day_ended` 被触发。
- 引擎会提取当日所有的 `urgent` 与 `batch` 事件，结合当前的情景记忆、社交关系和世界状态，构建一份“江湖日记”。
- 通过 `produce_daily_summary` 调用 LLM，以第一人称视角生成简短的事实性纪要（如：“游戏日 3：张三给了我一个馒头，随后我被李四攻击。”），随后清理过期的细粒度事件数据，极大地降低了长线存储和检索的成本。

### 2.3 降级与兜底策略
- **规则兜底**：如果 LLM 超时或报错，引擎不会阻塞，而是调用 `fallback_priority_split_error` / `fallback_priority_split_timeout`。它利用 YAML 中定义的 `EventTriagePreFilter` 优先级阈值（例如 `DeathNotification` 为 100，`TimeUpdate` 为 5），自动进行强规则分诊。
- **多数据源降级**：生成每日摘要时，如果缺少完整的 prompt 模板，会自动降级调用 `produce_event_summary` 进行纯事件回述。

## 3. 架构约束
- **主从隔离**：Triage 引擎的执行绝对不能阻塞 Agent 与 Server 之间的 WebSocket 通信。必须完全依赖 `tokio::spawn` 异步进行。
- **低成本**：要求采用较小的上下文和廉价的 LLM 模型完成分类，防止消耗大量 Token 预算。

## 4. 代码入口
- 分诊引擎: `crates/agent/src/component/immediate/session_triage.rs`
- 即时事件存储: `crates/agent/src/component/immediate/event_store.rs`