# 状态处理器 (StateProcessor)

**级别**: P0 核心基石
**模块**: `crates/server`

## 1. 设计目标
作为严格执行业务逻辑的核心管道，确保内存状态与数据库持久化的一致性，防止任何非法操作污染系统。

## 2. 核心机制
### 2.1 Saga 分布式事务模式
业务处理拆分为三个独立阶段：
1. **验证 (Validate)**：只读检查（是否有足够体力、目标是否在同一节点、技能冷却是否结束）。
2. **执行 (Execute)**：修改内存中的 `AgentState` 和 `WorldState`，返回修改增量（Delta）。
3. **持久化 (Persist)**：将增量通过 SQLx 写入 PostgreSQL。

### 2.2 失败回滚 (Rollback)
当第三步数据库持久化失败（如约束冲突、数据库宕机）时，系统利用 Saga 模式逆向执行 `rollback` 函数，将 DashMap 中的状态恢复原样，确保内存不出现“幽灵状态”。

### 2.3 死亡与掉落 (Death Physics)
- Agent 的 HP 归零触发死亡状态转移。
- 触发清空背包（调用 `InventoryManager::clear_inventory`）。
- 原有物品实例化为 `ground_items`（地面掉落物），散落在当前的 `node_id` 上，并附带存活时间（TTL），供其他存活的 Agent `pickup`。

### 2.4 物品消耗管线
- 统一了 `execute_use` 函数，处理所有类型消耗品（食物、饮水、丹药）的逻辑。
- 解析物品配置中的增益字典，对生理属性（如饱食度、水分）或战斗属性施加增益，并从背包中销毁物品。

## 3. 架构约束
- 所有状态变更（ActionExecutor）必须实现完整的 `Validate`, `Execute`, `Rollback` 接口。
- 绝对禁止越过 StateProcessor 直接修改 DashMap 中的 AgentState。

## 4. 代码入口
- 处理器定义: `crates/server/src/tick/processor/processor.rs`
- Saga 执行器: `crates/server/src/tick/processor/executor.rs`
- 事件处理: `crates/server/src/tick/processor/events.rs`
- 变异器: `crates/server/src/tick/processor/mutator.rs`
- 状态解析: `crates/server/src/tick/processor/resolver.rs`
- 技能变异: `crates/server/src/tick/processor/skill_mutator.rs`
- 模块入口: `crates/server/src/tick/processor/mod.rs`
