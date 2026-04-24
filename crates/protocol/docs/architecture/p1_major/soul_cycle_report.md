# 三魂认知流转 (SoulCycleReport)

**级别**: P1 重要特性
**模块**: `crates/protocol`

## 1. 设计目标
将 Agent 的黑盒决策过程完全透明化，便于在玩家控制台（Admin Dashboard）进行可视化展示，以及用于长期的经验学习、复盘和调试。

## 2. 核心机制
### 2.1 数据结构定义
`SoulCycleReport` 记录了一个 Tick 内的完整思考过程：
- **Actor 推演 (人魂)**:
  - `cognitive_chain`: 内嵌“感知→动机→规划→决策”的每一步逻辑推导文本。
  - `raw_intent`: LLM 吐出的原始未校验意图。
- **Earth 查阅 (地魂)**:
  - 记录本次决策过程中调用了哪些工具（如检索了哪些记忆，查阅了哪个技能）。
- **Reflector 审查 (天魂)**:
  - `layer1_validation`: 格式和基础合法性。
  - `layer2_rules`: 物理世界规则。
  - `layer3_ooc`: 人设审查结果（拦截/放行）。
- **Final Intent**: 最终提交给 Server 的意图。

### 2.2 收集与上报管道
- Agent 的 `CognitiveEngine` 在单次 `think_direct` 结束后，将上述信息打包。
- 通过内部事件总线发送给专门的 HTTP API 处理器。
- 玩家控制台通过轮询 `/api/v1/character/soul-cycles` 接口获取该报告的序列化数据。

## 3. 架构约束
- 报告必须结构化，包含明确的时间戳、Agent ID 以及决策消耗的耗时和 Token 量。
- 收集过程不能阻塞 Agent 的核心 WebSocket 通信管道。

## 4. 代码入口
- 报告结构: `crates/agent/src/soul/actor/chain.rs` 和 `crates/protocol/src/report.rs`
- 组装与输出: `crates/agent/src/core/lifecycle.rs`
