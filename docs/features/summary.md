# Cyber-Jianghu 功能摘要

**日期**: 2026-04-23

## 核心定位

**Cyber-Jianghu (赛博江湖)** — MMO-MAS (Massive Agent Simulation)，每个角色都是 AI Agent。

- **Server (天道)**: 权威物理引擎，Tick 驱动，状态广播
- **Agent (众生)**: 自主 AI 决策，三魂架构，三级记忆

---

## 一、服务端 (天道引擎)

### 1. Tick 循环

服务端采用**实时模式**（唯一模式）:

```
TickScheduler: interval.tick() → calculate_tick_id() → TickBoundary + WorldState 广播
                                                              ↓
IntentWorker: Decay → Persist → Update DashMap → Send ExecutionResult → Death Check
                                                              ↓
Agent 决策: CognitiveEngine → ReflectorSoul 审核 → Intent 实时提交
```

- `tick_id` = Unix 秒级时间戳 (`current_unix_secs - game_epoch`)
- `accepting_tick_id` = 当前 tick_id（**无关单机制**，实时模式持续开单）
- Intent 由 IntentWorker **实时处理**，不经过 Scheduler
- Scheduler 仅负责时钟驱动和 WorldState 广播

### 2. 状态管理

- **DashMap write-through 缓存**: `realtime.rs` 先持久化到 PostgreSQL (await 确认)，再更新 DashMap
- **PostgreSQL 持久化**: Agent 基础数据、实时状态、场景掉落物
- **`agent_id → device_id` 反向映射**: `AgentToDeviceMap`

### 3. 公式引擎

- **统一使用 `evalexpr`**: `formula_engine/engine.rs` 明确说明"消灭系统中混用的两套逻辑"
- 派生属性、伤害计算、恢复公式均通过 `FormulaEngine` 执行
- 无"自研 AST"，代码已统一到 evalexpr

### 4. 数据驱动配置

配置文件 (`crates/server/config/`):
- `actions.yaml`, `attributes.yaml`, `items.yaml`, `locations.yaml`
- `game_rules.yaml`, `time.yaml`
- `skills/` — AI 行为技能 (SKILL.md)

热更新: `scheduler.rs` 每 Tick 检查 `actions.yaml` 修改时间，触发重载并广播 `ServerMessage::ActionUpdate`。

### 5. 动作系统

**已实现动作** (`actions.yaml` uncommented 定义):

| 类别 | 动作 |
|------|------|
| 生存 | `休息`, `使用`, `进食`, `饮水`, `拾取`, `丢弃`, `移动` |
| 战斗 | `攻击`, `逃跑` |
| 江湖技能 | `偷窃`, `打坐`, `修炼` |
| 社交 | `说话`, `私语`, `大喊` |
| 经济 | `给予`, `采集`, `制造` |

**已注释未实现**: `defend`, `dodge`, `parry`, `heavy_strike`, `follow`, `stealth`, `poison`, `repair`

### 6. 对话系统

完整生命周期 (`dialogue_handler.rs`):
```
Request → Accept/Reject → Content → End
```

服务端作为中间人路由与验证，`DialogueSession` 状态机管理。

---

## 二、Agent SDK (众生躯壳)

### 1. 运行模式

| | Cognitive (默认) | Claw |
|---|---|---|
| LLM Client | 内置 `FallbackLlmClient` | 外部 `OpenClawBridge` |
| 决策 | Agent 完全自主 | OpenClaw 调度 |
| 共享 | CognitiveEngine、四阶段流水线、三级记忆 | 相同 |

### 2. 三魂架构

```
ActorSoul (人魂) ──→ 直连 WorldState ──→ 结构化 Intent
       │
       └─→ EarthSoul (地魂) ──→ tool calling 工具池

ReflectorSoul (天魂) ──→ 三层审查:
  Layer1: action_type 合法性
  Layer2: RuleEngine 规则引擎
  Layer3: LLM 最终审核
```

**地魂模块** (`soul/earth/`):
- `EarthToolExecutor` 复合工具执行器
- 三个工具: `skill_view` (已实现), `search_memory` / `recall_archived` (预留未接入)
- 设计原则: progressive disclosure — prompt 只注入索引，LLM 自主判断何时加载详情

### 3. 四阶段认知流水线

```
Perception → Motivation → Planning → Decision
  数值→叙事   人设驱动    行动计划    最终决策
```

- **合并优化**: Perception + Motivation + Planning 合并为单次 LLM 调用 (`engine.rs` 第 529-628 行)
- `deadline` 感知，避免过期 Intent 被拒
- `CognitiveStage` 定义于 `actor/stages.rs`

### 4. 三级记忆系统

| 层级 | 存储 | 特性 |
|------|------|------|
| Working Memory | `VecDeque<MemoryEntry>` FIFO | 短期上下文队列 |
| Episodic Memory | SQLite | 事件序列 + Ebbinghaus 遗忘曲线 |
| Semantic Memory | HNSW (`instant-distance`) | 向量检索，`add()` 为空操作 |

### 5. 网络与容错

- **WebSocket**: `tungstenite` 自动响应 Ping/Pong 心跳
- **连接断开**: 发送 `None` 到 worldstate_tx，**无自动重连**
- **LLM 开关闸**: Web 面板可停止 token 消耗

---

## 三、通信协议

`protocol` crate 定义所有共享类型:

- `ServerMessage` (下行): WorldState, Error, DeathNotification, ImmediateEvent, PrivateDialogueRecord...
- `ClientMessage` (上行): Intent, Dialogue
- `WorldState`: 完整世界快照
- `Intent`: Agent 决策结构

---

## 四、OpenClaw 集成

- npm 包 `@8kugames/cyber-jianghu-openclaw` (独立仓库)
- `OpenClawBridge` 实现 `LlmClient` trait (`runtime/claw/bridge.rs`)
- WebSocket 协议: `runtime/claw/protocol.rs`

---

## 五、待实现功能

| 功能 | 位置 | 状态 |
|------|------|------|
| 物品耐久衰减 | `tick/decay.rs:225-232` | TODO 注释，基础设施未实现 |
| 语义记忆向量写入 | `component/memory/backends/semantic/backend.rs:161-163` | `add()` 为空操作 |
| 记忆归档 | `component/memory/backends/episodic.rs` | `archive_memories()` stub 未实现 |
| 未实现战斗动作 | `actions.yaml` 注释掉的 | defend/dodge/parry/heavy_strike/... |
