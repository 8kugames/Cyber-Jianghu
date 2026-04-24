# Agent 对话会话 (DialogueSession)

**级别**: P1 重要特性
**模块**: `crates/protocol`

## 1. 设计目标
管理 NPC 间的点对点深度对话状态，而非简单的单向广播式“大喊”。使 Agent 能够建立多轮、有状态的沟通上下文。

## 2. 核心机制
### 2.1 五步流转协议
对话的生命周期由状态机管理，通过 `DialogueSession` 结构体进行流转：
1. **Request (请求)**：Agent A 发起对 Agent B 的对话邀请。
2. **Accept/Reject (接受/拒绝)**：Agent B 根据当前状态（是否在战斗中）与对 A 的好感度决定是否回应。
3. **Content (内容传递)**：建立连接后的多轮上下文互相传递。双方的 Memory System 会记录这段特定的对话。
4. **End (结束)**：任一方主动终止会话，或触发被动超时。

### 2.2 状态隔离与并发
- 对话流独立于主干 Action 物理管道，确保复杂的语言交互（需要等待 LLM 响应）不阻塞诸如移动、战斗等物理行为的 Tick 处理。
- 使用独立的 WebSocket Message Type（如 `ClientMessage::Dialogue`）承载。

## 3. 架构约束
- 必须包含严格的超时机制（Timeout），防止死锁的对话长期占用 Agent 资源和 Server 内存。
- 对话内容在最终落盘时，需汇总为 `Episodic Memory` 中的一个单一事件，而不是碎片化的每句话。

## 4. 代码入口
- 协议定义: `crates/protocol/src/dialogue.rs`
- Server 处理: `crates/server/src/actions/executor/interaction.rs`
- Agent 会话维护: `crates/agent/src/core/social.rs`
