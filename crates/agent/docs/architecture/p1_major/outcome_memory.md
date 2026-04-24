# 经验结果记忆 (Outcome Memory)

**级别**: P1 重要特性
**模块**: `crates/agent`

## 1. 设计目标
赋予 Agent 失败学习和经验总结的能力，打破“金鱼记忆”，避免其在同一个错误上反复栽跟头（如反复尝试推开一扇锁着的门）。

## 2. 核心机制
### 2.1 结果捕获
- 每当 Agent 提交 Intent 后，Server 或天魂（ReflectorSoul）会返回 `ExecutionResult`。
- 如果判定为失败（如距离过远、体力不足、目标不存在），该结果及其附带的中文 `GameError` 会被写入 `OutcomeMemory` 的 SQLite 库中。

### 2.2 前置规避注入
- 在下一次构建 `CognitiveEngine` 的 Prompt 时，系统会从 `OutcomeMemory` 中拉取最近的失败记录。
- 将这些教训作为 Context 的一部分（如：“【系统提示】你上次尝试攻击李四失败了，原因是：距离过远。请先移动到同一地点。”）注入给 LLM。
- LLM 结合此信息，自然会调整策略（如先执行 `move` 动作）。

## 3. 架构约束
- 必须能够将抽象的错误码转化为 LLM 易读的自然语言教训。
- 历史教训应当具有时效性，过久的失败记录不应干扰当前的决策。

## 4. 代码入口
- 内存组件: `crates/agent/src/component/memory/outcome.rs`
- 注入逻辑: `crates/agent/src/core/lifecycle.rs` (上下文组装)
