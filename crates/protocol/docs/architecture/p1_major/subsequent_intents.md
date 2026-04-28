# 多意图管道 (Subsequent Intents)

**级别**: P1 重要特性
**模块**: `crates/protocol`

## 1. 设计目标
允许 Agent 一次性提交包含后续动作的序列（Sequence），用于复杂连续行为的排队执行（如"走到地点 A -> 拾取物品 B"），减少大模型在短时间内的频繁调用，节省算力开销。

## 2. 核心机制

### 2.1 序列化 Intent 结构
在标准的 `Intent` 结构中，包含一个可选的 `subsequent_intents: Vec<Intent>` 字段。
Agent LLM 在规划阶段，可以一次性输出多个具有前后逻辑关联的动作。

### 2.2 Same-Tick 顺序执行
- Server 接收到带 `subsequent` 的 Intent 时，在同一个 Tick 内**立即顺序执行**主 Intent 及所有 subsequent intents。
- 执行顺序：主 Intent → subsequent_intents[0] → subsequent_intents[1] → ...
- 每条 Intent 执行完毕后才执行下一条，**不跨 Tick 排队**。
- 每条 Intent 执行前都经过 `StateProcessor` 的全套合法性校验（跨 Agent 校验如 attack/trade 也支持）。

### 2.3 自动中断机制 (Fail-fast)
若队列中的某一步动作执行失败（例如目标已经逃跑、被他人捷足先登、体力提前耗尽、持久化失败），后续的所有排队意图将**自动失效并中断**：
- 已成功的 Intent 保持成功（不回滚）
- 失败 Intent 向 Agent 发送 `ExecutionResult(success=false)`
- Pipeline 中断，Agent 收到通知后可重新规划

### 2.4 状态一致性
- 写入流程：执行 → Persist（DB） → 更新 DashMap → 发 ExecutionResult
- 若 Persist 失败：DashMap **不更新**，但已成功的 Intent 结果不受影响
- Agent 通过 `ExecutionResult` 区分该批中哪些 Intent 成功、哪些失败

## 3. 架构约束
- 不能滥用长序列导致环境变化带来的逻辑失效，通常通过 LLM Prompt 限制序列长度在 3-5 步以内。
- 每条 Intent 必须在执行前重新经历 `StateProcessor` 的全套合法性校验。
- Whisper（私语）session 在 Intent 失败时由 Server 自动清理，避免 session 泄漏。

## 4. 代码入口
- 意图定义: `crates/protocol/src/types/actions.rs` (`Intent::as_pipeline`)
- 意图处理: `crates/server/src/tick/realtime.rs` (`process_intent` + `process_single_subsequent`)
- 状态处理: `crates/server/src/tick/processor/processor.rs` (`process_single_intent`)
