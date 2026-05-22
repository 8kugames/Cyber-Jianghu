# 动作执行体系 (Action System)

**级别**: P0 核心基石
**模块**: `crates/server`

## 1. 设计目标
根据数据字典验证和执行具体的交互行为，支持解耦且可扩展的各类基础、战斗与交互动作。

## 2. 核心机制
### 2.1 动作分类
系统预定义了标准化的动作分类处理管线：
- **基础动作 (Basic)**：休息 (rest)、说话 (speak)、移动 (move)、大喊 (shout)、拾取 (pickup)、丢弃 (drop)、采集 (gather)、制造 (craft)、传授 (teach)。
- **战斗动作 (Combat)**：攻击 (attack)、使用物品 (use)、逃跑 (flee)。
- **交互/生活动作 (Interaction)**：给予 (give)、偷窃 (steal)、打坐 (meditate)、修炼 (practice)、私语 (whisper)。

> **设计决策: 为何没有 trade 动作**
>
> `trade` (两方协商交易) 在早期 PRD 中列出，但经审视后**主动移除**。
>
> 理由: 交易是社会行为而非物理动作。天道 (Server) 是物理引擎，不应裁决"公平交易"。
> 交易的涌现路径: A `give` B 支付 → B `give` A 货物 (或反序)。两次 `give` 之间**不存在原子性保证**——
> B 可以拿了钱不给货 (欺诈)。这正是设计意图: 信任、信誉、暴力讨债等社会机制应从这种脆弱性中涌现。
>
> 配置定义共 19 种动作 (`actions.yaml`)，其中 14 种有自定义 executor，其余走通用 effects 管线。

### 2.2 验证管线
所有动作都经过标准的验证宏或函数：
- 检查 `ActionType` 字符串是否在 `actions.yaml` 中注册。
- 检查 Agent 的前置条件（如不能在死亡状态下移动）。
- 检查目标实体是否存在，且距离合法（通常要求在同一 `NodeID`）。
- 检查体力/内力消耗是否足够。

### 2.3 数据驱动的属性结算
动作造成的伤害、消耗，均依赖 `formula_engine` 根据配置文件中的公式结合双方属性动态计算得出，而不写死在执行器中。

## 3. 架构约束
- 每个 Action 必须实现标准的 `Executor` Trait，解耦具体动作逻辑与主调度循环。
- 新增动作只需添加新的执行模块并注册路由，无需修改引擎主循环。

## 4. 代码入口
- 基础动作: `crates/server/src/actions/executor/basic.rs`
- 战斗动作: `crates/server/src/actions/executor/combat.rs`
- 交互动作: `crates/server/src/actions/executor/interaction.rs`
