# 动作执行体系 (Action System)

**级别**: P0 核心基石
**模块**: `crates/server`

## 1. 设计目标
根据数据字典验证和执行具体的交互行为，支持解耦且可扩展的各类基础、战斗与交互动作。

## 2. 核心机制
### 2.1 动作分类
系统预定义了标准化的动作分类处理管线（配置在 `actions.yaml` 中）：
- **基础动作 (Basic)**：休息 (rest)、说话 (speak)、移动 (move)、大喊 (shout)、拾取 (pickup)、丢弃 (drop)、采集 (gather)、制造 (craft)、传授 (teach)。
- **战斗动作 (Combat)**：攻击 (attack)、使用物品 (use)、进食 (eat)、饮水 (drink)、逃跑 (flee)。
- **交互/生活动作 (Interaction)**：给予 (give)、偷窃 (steal)、打坐 (meditate)、修炼 (practice)、私语 (whisper)。

> **设计决策: 为何没有 trade 动作**
> 
> `trade` (两方协商交易) 在早期 PRD 中列出，但经审视后**主动移除**。
> 理由: 交易是社会行为而非物理动作。天道 (Server) 是物理引擎，不应裁决"公平交易"。
> 交易的涌现路径: A `give` B 支付 → B `give` A 货物 (或反序)。两次 `give` 之间**不存在原子性保证**——
> B 可以拿了钱不给货 (欺诈)。这正是设计意图: 信任、信誉、暴力讨债等社会机制应从这种脆弱性中涌现。

### 2.2 验证管线与执行分离
动作执行被严格分为“意图解析验证”与“执行变异”两步：
- **验证**：检查 `ActionType` 注册、前置条件（如死亡限制）、目标距离（同节点）及资源消耗（体力/内力）。
- **执行转 StateChange**：`ActionExecutor` 并不直接修改 `AgentState`，而是输出代表变化的 `StateChange`（例如 `AttributeChanged`, `ItemAdded`）。
- **变异应用**：由一系列 `StateMutator` 将 `StateChange` 应用到内存状态或数据库中。

### 2.3 数据驱动的属性结算
动作造成的伤害、消耗，均依赖 `formula_engine` 根据配置文件中的表达式（evalexpr）结合双方属性动态计算得出，而不写死在执行器代码中。

## 3. 架构约束
- 每个 Action 的具体逻辑实现必须返回明确的 `StateChange`。
- 新增动作只需在 YAML 中注册配置，并在 Executor 侧添加对应逻辑生成变异即可。

## 4. 代码入口
- 执行器总管: `crates/server/src/actions/executor/mod.rs`
- 基础动作: `crates/server/src/actions/executor/basic.rs`
- 战斗动作: `crates/server/src/actions/executor/combat.rs`
- 交互动作: `crates/server/src/actions/executor/interaction.rs`
