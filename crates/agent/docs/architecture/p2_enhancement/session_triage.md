# 异步即时事件引擎 (SessionTriageEngine)

**级别**: P2 体验增强
**模块**: `crates/agent`

## 1. 设计目标
作为处理非 Tick 周期突发事件的后台大脑，接管 Server 发送的即时广播（如周围有人大喊、突发攻击），确保 Agent 不会漏掉关键信息，同时又不干扰主思考循环。

## 2. 核心机制
### 2.1 WAL 模式持久化
- 使用配置了 WAL (Write-Ahead Logging) 模式的 SQLite，确保接收到即时事件后立刻落盘不丢失，且读写并发性能极佳。

### 2.2 基于 LLM 的事件分类器 (分诊)
收到事件后，通过一个快速的 LLM 调用或规则引擎进行分类：
- **Urgent (立刻响应)**：如有人喊自己名字、遭受攻击。立即注入 Agent 的 `Working Memory` 顶部，供下一轮主决策循环立刻处理。
- **Batch (稍后批处理)**：如远处的无关战斗、背景噪音。存入暂存表，收集并在当前游戏日结束时打包。
- **Ignore (忽略)**：完全无意义的系统噪音，直接丢弃，不污染记忆库。

### 2.3 每日总结提取
- 游戏日结束时（触发条件），提取 Batch 队列中的事件。
- 调用 `produce_daily_summary` 生成一段总结性的文本，随后写入 `Episodic Memory`。

## 3. 架构约束
- 异步处理绝对不能阻塞 Agent 的主 WebSocket 通信和 Tick 决策循环。
- 分类器调用 LLM 时应采用低延迟、低并发的小模型（如 Qwen 7B）。

## 4. 代码入口
- 分诊引擎: `crates/agent/src/component/immediate/session_triage.rs`
