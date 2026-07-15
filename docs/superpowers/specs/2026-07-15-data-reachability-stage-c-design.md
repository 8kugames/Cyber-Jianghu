# C 阶段：数据可达性设计

> 日期：2026-07-15
> 分支：`pure`
> 状态：方向经 3-agent 投票 3/3 通过；关系同步策略经 3-agent 投票 3/3 通过（策略 B 全量快照）
> 前置：A 阶段（数据诚实）已完成，10 个 commit 落地，编译+测试全绿

## 1. 使命与定义

**使命**：让前端可直接通过 API 获取完整数据。

**"可达"的定义**：数据在权威存储 + 有端点暴露 + 有鉴权可达。A 阶段解决了"数据在权威存储且诚实"，C 解决"有端点暴露且有鉴权可达"。

**B 阶段（数据不丢）已证伪为伪需求**（3/3 投票）：权威状态写穿+原子化（A 成果），B 候选四项（消息可靠性/Chronicle 事务/SoulCycleRecorder/迁移器）均不阻塞"前端取数据"。

## 2. 物理约束（代码验证）

- 权威游戏状态在 PostgreSQL，写穿+原子化（A 成果，4 表同 tx）
- 对话/chronicle/经历已在 DB，有端点（`/api/dashboard/experiences` 原样返回 action_data JSONB）
- **关系图谱在 agent 本地 SQLite，server DB 零关系表——数据真空**
- agent→server 有统一 WS 通道（intent_tx mpsc，容量 32），DailySummary 已搭便车
- 鉴权有三档（admin_read/admin_write/device），全部 Bearer Header，无游戏客户端档
- dashboard 全部 READ 端点用 `require_read_token` layer（admin R 或 RW token）
- 1 游戏日 ≈ 12 分钟现实时间（tick 60s × 12 tick/日）

## 3. 五件事（执行顺序有硬依赖）

### C0：进程内迁移器（部署地基）

**问题**：21 个 migration 外部应用（`sqlx migrate run`），无 `sqlx::migrate!`。C1 建新表若部署漏跑，端点报错。

**方案**：`sqlx::migrate!()` 宏嵌入 server 启动流程。server 启动时自动跑 migration，不依赖外部步骤。

**为什么在 C 第一**：C1 要建 `agent_relationships` 表（migration 022），若迁移器没就绪，部署顺序错误会遮蔽端点。这不是 B 的"可靠性"，是 C 的"部署地基"。

### C1：关系图谱存储 + 同步 + 端点（最大缺口）

**问题**：关系图谱（好感度/认识关系/关键事件）在 agent 本地 SQLite，server crate 零关系代码，前端物理上取不到。A 阶段已在 protocol 定义 `RelationshipMemory`/`RelationshipKeyEvent` 契约。

**同步策略：B 全量快照上报**（3/3 投票通过）

每游戏日结束时，agent 把完整关系快照（`get_all_relationships()`）上报给 server。server 全量覆盖（UPSERT），天然幂等。

**为什么不用增量（策略 A）**（3/3 一致证伪）：
- favorability 是不可重放的累加 clamp 值（`relationship.rs:473`）
- WS 断连时 intent_tx=None，消息必丢（`websocket.rs:686`）
- agent 无上报水位，丢 delta 后 server 永久偏离真值，无法自愈
- key_events FIFO 只留 20 条，断连期间超出的事件物理消失

**为什么全量快照可接受**（3/3 一致确认）：
- 1 游戏日 ≈ 12 分钟现实时间，关系图谱是慢变量展示数据，延迟无感知
- 天然幂等：断连下次重报即恢复，server 必然收敛
- 复刻 DailySummary 已验证范本（`upsert_agent_daily_summary` 的 `ON CONFLICT DO UPDATE`）
- 转世重生用新 agent_id 开新空关系，全量覆盖语义天然兼容

**实施四件**：
1. **protocol**：新增 `ClientMessage::RelationshipSnapshot { agent_id, game_day, relationships: Vec<RelationshipMemory> }`。复用 A 阶段已定义的 `RelationshipMemory`（时间戳 i64 毫秒）。注意类型转换：agent 本地 `DateTime<Utc>` → protocol `i64`（timestamp_millis）。
2. **agent 端**：在游戏日结束钩子（与 DailySummary 同批，`session_triage.rs` 的 game_day 边界）调 `get_all_relationships()` 序列化上报。复用 intent_tx 通道，每游戏日发一次。
3. **server 端建表**（migration 022）：`agent_relationships` 表（source_agent_id, target_agent_id, target_name, favorability CHECK[-100,100], last_interaction_tick, synced_at, self_description, description_tick）+ `agent_relationship_key_events` 子表。UPSERT 键 `(source_agent_id, target_agent_id)`，key_events 全量覆盖（DELETE+INSERT，镜像 agent 本地 `upsert_relationship` 语义）。
4. **server 端 handler**：`handle_client_message` 加 `RelationshipSnapshot` 分支（仿 `handle_daily_summary`）。`GET /api/dashboard/agent-relationships`（全量）+ `GET /api/dashboard/agent-relationships/{agent_id}`（单 agent 的所有关系）。

### C2：游戏客户端鉴权档

**问题**：当前 dashboard READ 端点用 `require_read_token`（admin R/RW token）。前端必须用 admin token（全局共享静态串，过度特权）。

**方案**：新增 `client_read_token` 配置项（仿 admin_read_token），新增 `require_client_read_token` middleware（接受 client_read_token **或** admin_read_token）。dashboard READ 端点的鉴权 layer 改为接受两者。

**为什么不新建 clients 表 / JWT**：当前无 player 概念（前端是观察者/管理者视角，不是玩家登录）。新建 clients 表 + JWT 是为不存在的需求建基础设施（臆造）。最简方案是配置项 + middleware，与 admin 模式对称。

> 这是本设计里唯一未经投票的决策点。若执行时认为需要更细的身份区分，起 3-agent 投票。

### C3：统一世界快照端点

**问题**：数据碎片化。前端要拼一次世界视图需打多个端点（agents/stats/health/emergence/experiences）。

**方案**：新增 `GET /api/dashboard/world-snapshot`，一次返回 `{agents, world_time, tick_info, recent_events, deaths}` 聚合。

**正确性要求**（投票 agent 2 发现）：读路径用 `READ ONLY` 事务隔离，消除 tick 边界瞬时跨 agent 不一致（部分 agent 已推进 tick、部分未推进）。当前 `get_all_alive_agents_latest_states` 是裸 SELECT 无事务。

### C4：缺口端点 + 顺手修复

- **地点/地图端点**：`GET /api/dashboard/locations`（从 LocationRegistry 读节点+边图）
- **死亡时间线**：`GET /api/dashboard/deaths`（从 agent_action_logs 查死亡事件，按时间线）
- **对话聚合视图**：`GET /api/dashboard/dialogues?agent_a=&agent_b=`（从 action_logs 聚合 speak/whisper，双向拼接）
- **chronicle 幂等顺手修**：migration 补 `UNIQUE(period_start, period_end)` + `ON CONFLICT DO UPDATE`（防重复行）
- **context.rs 占位符修复**：`context.rs:105,175` 的硬编码 `"(查看物品详情需要额外查询)"` 改为查真实库存

## 4. 明确不做（排除清单）

| 排除项 | 归属/理由 |
|--------|----------|
| 消息交付可靠性 / outbox | B，咨询性抖动已证伪 |
| Chronicle 事务/幂等/回补（除顺手 UNIQUE） | B，派生可重算 |
| SoulCycleRecorder 返回 Result | B，训练数据非前端数据 |
| 增量关系同步（策略 A/C） | 已证伪，权宜+致命丢增量 |
| 关系实时推送（WS） | 慢变量展示数据，12 分钟延迟无感知 |
| player 登录系统 / JWT | 臆造需求，当前无 player 概念 |

## 5. 验证策略

- **编译期**：枚举化穷尽匹配（A 成果延续）
- **DB 约束**：favorability CHECK[-100,100]，UNIQUE(source_agent_id, target_agent_id)，chronicle UNIQUE(period_start, period_end)
- **集成测试**：C1 关系同步的端到端测试（agent 生成快照 → WS 上报 → server 存储 → API 读取 → 数据一致）
- **幂等测试**：同一快照重报不产生重复行
- **每 commit 编译+测试验证**

## 6. 实施顺序（硬依赖链）

```
C0 迁移器（部署地基）
  ↓ C1 建表依赖 migration 就绪
C1 关系存储+同步+端点（最大工程）
  ↓ C1 端点暴露需要鉴权
C2 鉴权档
  ↓ 其余端点暴露需要鉴权
C3 世界快照（含读路径事务隔离）
  ↓
C4 缺口端点 + 顺手修复
```
