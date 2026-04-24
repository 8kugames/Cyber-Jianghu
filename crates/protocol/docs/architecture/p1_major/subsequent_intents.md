# 多意图管道 (Subsequent Intents)

**级别**: P1 重要特性
**模块**: `crates/protocol`

## 1. 设计目标
允许 Agent 一次性提交包含后续动作的序列（Sequence），用于复杂连续行为的排队执行（如“走到地点 A -> 拾取物品 B”），减少大模型在短时间内的频繁调用，节省算力开销。

## 2. 核心机制
### 2.1 序列化 Intent 结构
在标准的 `Intent` 结构中，包含一个可选的 `subsequent_intents: Vec<Intent>` 字段。
Agent LLM 在规划阶段，可以一次性输出多个具有前后逻辑关联的动作。

### 2.2 Server 端排队与调度
- Server 接收到带 `subsequent` 的 Intent 时，首先执行主 Intent。
- 将剩余的 `subsequent_intents` 挂载到该 Agent 的 `AgentState` 中的 `intent_queue` 字段。
- 在接下来的几个 Tick 周期中，`TickScheduler` 会自动从队列弹出动作并送入 `IntentWorker` 处理，直至队列清空。

### 2.3 自动中断机制 (Fail-fast)
若队列中的某一步动作执行失败（例如目标已经逃跑、被他人捷足先登、体力提前耗尽），后续的所有排队意图将**自动失效并中断**，并触发 ExecutionResult 通知 Agent 重新规划。

## 3. 架构约束
- 不能滥用长序列导致环境变化带来的逻辑失效，通常通过 LLM Prompt 限制序列长度在 3-5 步以内。
- 队列中的 Intent 必须在每个 Tick 重新经历 `StateProcessor` 的全套合法性校验。

## 4. 代码入口
- 意图定义: `crates/protocol/src/action.rs`
- 队列调度: `crates/server/src/tick/realtime.rs` (排队逻辑)
