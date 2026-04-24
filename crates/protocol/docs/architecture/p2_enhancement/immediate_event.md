# 即时事件广播 (ImmediateEvent)

**级别**: P2 体验增强
**模块**: `crates/protocol`

## 1. 设计目标
绕过传统 Tick 时钟周期的即时消息通道，专用于处理需要立刻感知的高时效性行为（如周围有人说话、突发攻击预警），增强 Agent 响应的灵敏度。

## 2. 核心机制
### 2.1 通道分离与推流
- 在 Server 端的 WebSocket 架构中，建立独立于 `WorldState` 周期广播的即时消息分发逻辑。
- 发生即时事件（如某 Agent 执行了 `speak` 动作）后，Server 立即构造 `ServerMessage::ImmediateEvent` 并推送到同 `node_id` 下所有 Agent。

### 2.2 LLM 认知融合与分诊
- Agent 端收到事件后，送入 **异步即时事件引擎 (SessionTriageEngine)**。
- 引擎通过轻量级 LLM 或规则将其分类。如果判定为“紧急 (urgent)”，则在下一次决策循环前，强行插入工作记忆（Working Memory）的顶部，确保立刻被关注。

## 3. 架构约束
- 仅限极少量高频交互动作（语音、突发攻击）使用。
- 禁止滥用以防破坏 Tick 物理引擎的权威性，且过多的 ImmediateEvent 会打断 Agent 正在进行的序列动作。

## 4. 代码入口
- 协议定义: `crates/protocol/src/messages.rs`
- Agent 分诊引擎: `crates/agent/src/component/immediate/session_triage.rs`
