# 状态处理器 (StateProcessor)

**级别**: P0 核心基石
**模块**: `crates/server`

## 1. 设计目标
协调 Agent 动作的解析、状态计算（Mutator）、结果校验及数据库日志落盘。这是确保 Agent 的 Intent 转化为合法的系统数据变化的核心组件。

## 2. 核心机制
### 2.1 验证与执行分离
- **解析器 (IntentResolver)**：只读检查（参数合法性、目标是否同节点、CD/资源检查）。
- **执行器 (ActionExecutor)**：根据 `ActionType` 调度不同的执行子模块（Basic, Combat, Interaction 等），计算得到状态变更增量集合 `StateChange`。
- **变异器 (StateMutator)**：将 `StateChange` 应用到内存中的 `AgentState` 上，包括属性变异、物品扣除与新增、位置移动及技能变异。

### 2.2 内存快照回滚
- 在处理开始前，先建立当前 `AgentState` 的克隆快照。
- 在变异器链条应用过程中，如果任何一步失败或触发异常，将直接使用克隆的快照进行状态覆盖，丢弃部分计算结果，确保不会有“半执行”的状态泄漏。
- 这是进程内存级别的回滚，数据库级的失败处理由上层 `IntentWorker` 通过 Write-Through 策略来解决。

### 2.3 动态技能习得与观察学习
- `StateProcessor` 会在动作成功后累加 Agent 的 `action_counts`。
- 检测如果某类别动作累计达到配置文件中的阈值，则自动下发 `SkillLearned` 状态变更，赋予 Agent 对应的新 AI 技能（SKILL.md）。
- 制造物品（Crafting）成功时，同节点的旁观者会累积“观察计数”，当达到一定次数后，旁观者也会自动习得该配方（Recipe Observation）。

### 2.4 Action Log 异步落盘
- 所有的意图执行结果（无论成功与否），都会被封装为 `AgentAction`，包含失败原因、思考日志、验证反馈等，被异步投递到数据库中（`agent_action_logs`表）。这是后续生成传记与 Agent 反思的依据。

## 3. 架构约束
- 变异器链条必须是确定性的，禁止包含任何随机数或外部 I/O 阻塞。
- 禁止跨模块直接修改属性，一切改变必须走 `StateChange` 枚举。

## 4. 代码入口
- 处理器定义: `crates/server/src/tick/processor/processor.rs`
- 变异器接口: `crates/server/src/tick/processor/mutator.rs`
- 解析与校验: `crates/server/src/tick/processor/resolver.rs`
- 技能变异: `crates/server/src/tick/processor/skill_mutator.rs`
