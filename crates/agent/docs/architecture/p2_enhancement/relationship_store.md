# 人际社交网络 (RelationshipStore)

**级别**: P2 体验增强
**模块**: `crates/agent`

## 1. 第一性原理与设计目标
在 MMO-MAS 中，社会关系的涌现需要持久化支撑。由于 Agent 存在于分布式的进程中，社会图谱不能是一个全局唯一的图数据库，而必须是**每个 Agent 主观视角下的离散记忆网络**。
`RelationshipStore` 的目标是量化 Agent 与他人的互动历史，将其抽象为好感度和关键事件，并在每个 Tick 决策时将这些社会关系注入到工作记忆中，影响其行为动机。

## 2. 核心机制

### 2.1 社交事件记录与好感度计算
- 使用 `rusqlite` 将每个目标的互动数据落盘至 `relationships_{agent_id}.db`。
- **事件追加**：通过 `record_social_event` 记录每次社交动作（如交易、攻击），更新 `favorability`（-100 到 100 之间 Clamp），并采用 `INSERT OR REPLACE` 和级联清理逻辑保持 `key_events` 的数量上限（默认 20 条），避免无限膨胀。

### 2.2 认知上下文反哺
- 当 Agent 的视野（WorldState）中出现其他实体时，通过异步调用 `maybe_update_narratives`（或由 `NarrativeGenerator` 进行防抖更新），提取目标的好感度和历史。
- 将离散的数值映射为自然语言文本（如“友善”、“敌对”），并将其作为 `memory_context` 的一部分注入，使得 LLM 在推理（Perception）阶段能够自然地理解“面前这个人是仇人还是朋友”。

### 2.3 动态人设与观念演变
- 配合 `DynamicPersona`，长期且高权重的社交互动事件会触发角色世界观或性格标签的变迁。关系记忆不仅作为客观事实存在，更是塑造人格的数据源。

## 3. 架构约束
- **单写多读与并发安全**：SQLite 的连接通过 `Arc<Mutex<Connection>>` 保护（因为 `rusqlite::Connection` 是 Send 但非 Sync）。必须尽量缩短锁的持有时间。
- **无状态重构**：存储结构必须足够扁平化。复杂的查询（如按好感度过滤）应下推至 SQL 层完成，严禁将全表数据拉入内存后再进行过滤。

## 4. 代码入口
- 关系存储: `crates/agent/src/component/social/relationship.rs`
- 关系类型定义: `crates/agent/src/component/social/relationship_types.rs`