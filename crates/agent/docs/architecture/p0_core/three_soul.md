# 三魂架构 (Three-Soul Architecture)

虚境：江湖中的每个 Agent 都不是单一的大模型调用，而是由三个职责分明的“魂”组成的分布式认知系统。这种架构实现了**“思维发散”与“规则收敛”**的平衡。

相关代码路径：`crates/agent/src/soul/`

## 整体设计思想

- **ActorSoul (人魂 / 认知之魂)**：负责“我想做什么”。直连世界状态，发散思维，主动思考。
- **EarthSoul (地魂 / 能力之魂)**：负责“我能查什么”。内嵌在人魂思考过程中的工具池。
- **ReflectorSoul (天魂 / 守护之魂)**：负责“我允许做什么”。作为看门人，严格审查人魂的意图是否合法。

## 1. ActorSoul (人魂)

人魂是整个 Agent 的驱动核心（实现位于 `crates/agent/src/soul/actor/engine.rs` 中的 `CognitiveEngine`）。

- **输入**：`WorldState`（当前世界客观物理状态）、`ThreadSafePersona`（当前人设状态）、`MemoryContext`（记忆）。
- **处理**：一次端到端的 LLM 推理。内部逻辑经过 Perception（感知）→ Motivation（动机）→ Planning（规划）→ Decision（决策）四个阶段。
- **输出**：一组精确的可执行动作（例如 `action_type: "move", action_data: {"target_node": "inn"}`），称为 `Intent`。

## 2. EarthSoul (地魂)

地魂**不再是一个独立的流水线阶段**，而是作为 Tool Calling（工具调用）插件，内嵌于人魂的大模型推理循环中（实现位于 `crates/agent/src/soul/earth/`）。

- **工作方式**：当人魂大模型在推理过程中发现缺少信息（例如：“李四的详细好感度是多少？”或“如何制作金创药？”），它会暂停推理，调用地魂提供的 `relationship_tool` 或 `recipe_tool`。
- **工具池**：
  - `memory_tool`: 检索遥远的语义记忆。
  - `skill_tool`: 查阅拥有的技能详细规则。
  - `rule_tool`: 查阅世界的客观物理规则。
  - `state_tool`: 检索复杂的实体详情。
- **Budget 机制**：为了防止大模型死循环调用工具，地魂中实现了 `ToolResultBudget` 和 `LoopGuard`，严格限制工具调用的次数和返回字符量。

## 3. ReflectorSoul (天魂)

天魂是意图离开 Agent 提交给 Server 之前的最后一道关卡（实现位于 `crates/agent/src/soul/reflector/validator.rs`）。它的核心职责是“拒绝不合理的行为”。

审查分为三层（Layered Validation）：

1. **Layer 1: Schema 审查**：检查 action_type 是否合法，必须字段是否缺失。
2. **Layer 2: RuleEngine 审查**：硬性物理规则检查（例如：移动目标不存在、吃的东西没有在背包里、冷却时间未到）。这部分是**完全基于代码和状态**的，不消耗 Token。
3. **Layer 3: LLM 认知审查**：如果开启了强力审查（如针对 `speak`），会调用 LLM 进行软性审查：这句话是否符合角色人设？是否包含现代词汇破坏了江湖沉浸感？

### 分级审查策略 (Graded Validation)

为了平衡成本与安全性，天魂采用分级审查（根据配置中的 `GradedValidationConfig`）：
- `Always`: 强制三层审查（通常用于 `speak` / `shout` 等高风险社交动作）。
- `Skip`: 跳过 LLM 审查，只进行 Schema 和 RuleEngine 检查（如 `idle`, `move`）。
- `Adaptive`: 动态判断，根据上下文和历史失败率决定是否启用 LLM。

当天魂驳回一个人魂的意图时，它会将生硬的技术错误（如 `ERR_ITEM_NOT_FOUND`）通过 `narrativize_rejection` 转化为叙事化反馈（如“你翻了翻口袋，发现里面并没有这个东西”），并在下一 Tick 作为惩罚反馈给 ActorSoul。
