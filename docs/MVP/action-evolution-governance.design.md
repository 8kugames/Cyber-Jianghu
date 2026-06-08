# 动作演化治理方案（伏羲审议链）

## 1. 文档目的

本文定义一套完整、可落地、可审计、可回滚的“动作演化治理”方案，用于实现以下目标：

- 天魂保持现有拒绝行为，不放松执行安全边界。
- 当 Agent 因未知动作或现有动作表达力不足而被拒绝时，异步触发一次自评估。
- 只允许提审原子行为，复合行为直接在提审前被丢弃（不予通过）。
- 若自评估判断该需求真实存在且为原子行为，则向 Server 发起“动作演化提案”。
- 由伏羲周期性审查提案：
  - 对于可自动演化的白名单部分，生成规则化 action 配置并热更新广播。
  - 对于不可自动演化但确有必要的部分，进入 server-admin 提案页，由运营开发纳入版本开发。

本文不是临时补丁设计，而是从“谁有权改变世界规则、谁负责执行世界规则、谁只负责提出需求”三个第一性问题出发，重新定义动作演化的治理链路。

## 2. 用户目标

用户预期目标可以被严格表达为：

1. 保持现有三魂执行链路不变，未知动作仍然拒绝。
2. 拒绝不终止信号价值，而是转化为演化提案。
3. **只允许提审原子行为 action，不允许提审复合行为 action，判定为复合 action 的不予通过。**
4. 自动演化必须以数据驱动、配置驱动为核心，不允许靠硬编码补洞。
5. 对无法自动演化但确有必要的新能力，必须进入可审阅、可排期、可跟踪的人类开发流程。

这五点必须同时成立。缺任何一点，方案就偏离原始目标。

## 3. 第一性原理

### 3.1 世界规则修改权必须属于 Server

根源问题不是“如何让 Agent 更聪明”，而是“谁拥有修改世界法则的权力”。

- Agent 的职责：感知、决策、提案。
- Server 的职责：验证、执行、治理、广播。
- 配置的职责：表达当前允许存在的世界规则。

如果允许 Agent 直接落配置，系统就不再是 server authoritative。

### 3.2 执行链与治理链必须分离

执行链回答的是：这个动作当前是否允许执行？
治理链回答的是：这个动作未来是否值得纳入世界法则？

因此必须坚持：

- **执行链严格拒绝**
- **治理链异步审议**

### 3.3 数据驱动的真实边界必须说清

当前项目的动作系统是“**半开放的配置驱动系统**”，不是“任意新语义可自动涌现系统”。
真正的一等事实不应是“文档里手写了哪些允许字符串”，而应是：

- Server 当前加载出的动作 schema 能力
- Server 当前执行器实际暴露的能力清单
- 由两者合成的**能力注册表（Capability Manifest）**

本文后续凡是出现“允许维度”“禁止维度”“能力分组”等概念，均以 Capability Manifest 为唯一机器真源。

### 3.4 原子性先于演化

复合行为（如“交易”、“结盟”）本质上是多个主体、多个状态机、多个原子动作的协议编排。
把复合行为压缩成单个 action 会导致：

- 隐藏子事务，破坏单一动作结算原则。
- 引入新的状态机，超出配置驱动的表达边界。

因此，**必须在提案阶段通过机器可判定的 IR (Intermediate Representation) 进行原子行为过滤，拒绝所有复合行为。**

## 4. 术语定义

### 4.1 被拒绝动作

指 ActorSoul 产出的动作，在天魂校验阶段因以下原因被拒绝：

- 当前 `actions` 中不存在该动作
- 当前动作意图可被识别，但缺少世界级支持

### 4.2 动作演化提案 IR (ProposedActionIR)

指由 Agent 在拒绝后异步产生的一份**类型化中间表示**，而不是自然语言散文。
它严格描述了行为的执行特征（参与者数量、阶段数、时间跨度、副作用数量）。

### 4.3 原子行为

通过 ProposedActionIR 机器可判定的单一不可分割行为。必须满足：

- `actor_arity == 1`（单一发起者）
- `phase_count == 1`（无多阶段协议）
- `tick_span == 0`（单 tick 结算）
- `protocol_kind == none`（非流程编排或双边组合）

反例：

- `交易`：包含双方给予，`protocol_kind != none`，不是原子行为。
- `拜师收徒`：包含多阶段确认，`phase_count > 1`，不是原子行为。

**裁决：复合行为不予通过，不允许提审。**

## 5. 总体架构

### 5.1 总体链路

```text
ActorSoul 产出动作 -> ReflectorSoul 校验 -> 拒绝
    ->
异步触发 Rejection Self-Evaluator (生成 ProposedActionIR)
    ->
执行原子行为判定 (基于 IR)
    ->
若为复合行为：直接 Drop (不予通过)
若为原子行为且真实需要：提交 ActionEvolutionProposal 到 Server
    ->
Server Proposal Aggregator 聚合相似提案
    ->
Fuxi Review Worker 周期审议
    ->
白名单内：生成 ActionConfig 草案 -> staged 校验 -> 原子热更新 -> 广播 ConfigUpdate
白名单外但必要：转入 server-admin 提案页
```

## 6. Agent 侧设计：Self-Evaluator 与原子闸门

### 6.1 拒绝信号契约：RawRejectionFact 与 GovernanceCode

为了防止 Agent 越权或漏采信号，协议拆分为两层：

1. **RawRejectionFact**:
   - Reflector 或 Server 执行器只产出原始事实（哪个动作、缺什么参数、哪个校验失败）。
2. **GovernanceCode**:
   - Server 治理入口统一将 RawRejectionFact 映射为治理分类码（如 `unknown_action`, `expression_gap`, `non_governance_reject`）。

Agent 的 Self-Evaluator 仅在收到映射后的 `unknown_action` 或 `expression_gap` 才会触发。普通参数错误等被直接排除。

### 6.2 自评估与 IR 生成

Self-Evaluator 接收拒绝事实和上下文，不输出散文，而是输出严格的 `ProposedActionIR` 和决策：

```json
{
  "decision": "drop | use_existing | propose",
  "ir": {
    "actor_arity": 1,
    "target_arity": "0_to_many",
    "tick_span": 0,
    "phase_count": 1,
    "protocol_kind": "none",
    "state_transition_count": 1,
    "effect_refs": ["..."],
    "requirement_refs": ["..."]
  },
  "rationale": "string"
}
```

### 6.3 原子行为闸门 (硬拦截)

Self-Evaluator 产出 IR 后，必须通过本地/服务端的原子行为函数校验：

```rust
fn is_atomic(ir: &ProposedActionIR) -> bool {
    ir.actor_arity == 1 &&
    ir.tick_span == 0 &&
    ir.phase_count == 1 &&
    ir.protocol_kind == ProtocolKind::None
}
```

**如果 `!is_atomic(ir)`，则 `decision` 强制被覆写为 `drop`，不予通过，不允许发送至 Server 提案队列。**

## 7. Server 侧设计

### 7.1 提案表与主载体

- `action_evolution_proposals`：只存 raw evidence。
- `action_evolution_proposal_groups`：治理状态机的主载体，绑定 issue、版本切换和关闭条件。

### 7.2 能力注册表：Capability Manifest (事实层)

Server 启动时由各执行器自动投影生成。
仅包含：`capability_id`, `kind`, `semantic_scope`。
**不包含策略**（如 `is_auto_evolution_allowed`），策略交由 `game_rules.yaml` 配置，避免硬编码回潮。

### 7.3 自动演化白名单 (策略层)

白名单通过 `game_rules.yaml` 定义：

```yaml
action_evolution:
  capability_policy:
    allowed_capability_groups: ["basic_attributes", "basic_items"]
    denied_capability_ids: ["instant_kill", "force_trade"]
```

白名单判定只比对 IR 中的 `effect_refs` 是否在允许的 Manifest ID 内。

### 7.4 复合提案拆解闭环

因为前置闸门已经**不允许复合行为提审**，进入到 Fuxi 审查的提案理论上全是原子的。
但如果 Fuxi 审查发现 Agent 的 IR 撒谎（例如名为“交易”，IR 伪装成原子），Fuxi 必须将其状态标记为 `rejected_composite`。

## 8. 热更新与收敛协议

### 8.1 唯一真源切分

- **Phase 0**: `actions.yaml` 仍是真源。不允许自动写配置。
- **Phase 1+**: 统一切到 `DB 真源 (action_config_versions) + yaml 快照导出`。禁用人工绕过 Pipeline 直接改文件。

### 8.2 协议收敛与 ACK

热更新后，Agent 必须通过新增的 `config_applied_ack` 消息反馈。

- 只有本地 `actions_version` 且 prompt cache 切到新版本后，Agent 才允许发 ACK。
- Server 冻结一个 `rollout target set`。满足多数 ACK 后，Proposal 状态机才能进入 `converged -> closed`。

### 8.3 状态机闭环

错误恢复被收敛：

- 所有中间失败落入 `error`，带上 `error_code`。
- 回滚采用**前滚式回滚**（生成 `V+2` 内容同 `V`），保证版本单调，不破坏 ACK 语义。

## 9. 实施阶段：Phase 0 冻结

当前 MVP **仅交付 Phase 0**：

- **In Scope**: 产生 RawRejectionFact，映射 GovernanceCode，Self-Evaluator 生成 IR，执行原子闸门拦截，落库，离线报表。
- **Out of Scope**: Fuxi 自动审查、自动写配置、热更新、Admin 工作流。

这保证了当前 MVP 边界清晰，且满足“不提审复合行为”的最终约束。
