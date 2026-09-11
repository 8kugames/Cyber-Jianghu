# 地点图与移动语义（唯一事实文档）

**级别**: P0 核心机制
**模块**: `crates/protocol`（契约）+ `crates/server`（执行）

> 本文件是地点图数据语义的**唯一事实文档**（模式吸收自传灯录
> `docs/map-coordinate-system.md`）。travel_cost 单位、隐式边规则、
> 时代显隐语义的任何改动，必须与本文及对应实现同步更新。

## 1. 图模型

- **节点**（`locations.yaml` `data.nodes`，协议类型 `LocationNode`）三层：
  `region`（区域）→ `map`（地图）→ `sub_scene`（子场景），经 `parent_id` 层级引用。
- **边**（`data.edges`，`LocationEdge`）为**有向声明**：
  - `travel_cost` 单位 = **tick 数**（非游戏小时、非现实秒）。
    体力消耗 = `travel_cost * game_rules.agent_state.location.travel_stamina_multiplier`。
  - A→B 与 B→A 是两条独立声明：**允许不对称**（"回程更累"）。
    当前 shipped 画像为全双向（见 §4 守卫）。
- **隐式 parent-child 边**：`sub_scene`/`map` 与其 `parent` 自动互达，
  cost 取节点 `implicit_travel_cost`，未配置时用全局
  `game_rules.agent_state.location.default_implicit_travel_cost`。
  显式边已存在时优先显式 cost（去重）。

## 2. 移动语义

- 移动为**单跳**：`execute_move` 校验 `is_connected(current, target)`
  （显式边 + 隐式 parent-child，自环合法）。**无多跳寻路**——
  Agent 需自行沿 WorldState 的 `adjacent_nodes` 逐步规划路线
  （涌现式寻路，与 AI Agent 自主决策理念一致）。
- 起点不校验时代可见性（所在地永远允许离开，与传灯录同语义）。

## 3. 时代显隐（time_variants）

`LocationNode.time_variants`（可选，additive，`PROTOCOL_VERSION >= 3.1.0`）：
按**游戏日闭区间**控制显隐与描述，语义与传灯录 `time_variants` 对齐：

- 区间 `[from_game_day, to_game_day]` **两端含**；缺省边界 = 无下/上界；
  游戏日 1-based，单调递增，由 `TimeRegistry::game_day(tick_id)` 统一换算
  （`game_hours / hours_per_day + 1`，executor 与 broadcaster 同源）。
- 列表**首个命中**生效；全部未命中回落节点基础定义。
- `visible: false` = 该时段隐藏：
  - **不可入**：`execute_move` 拒绝（"当前时代不可达"）；
  - **不可见**：WorldState `adjacent_nodes` 过滤（Agent 认知中不存在）；
  - **起点豁免**：身处隐藏地仍可离开。
- `description` 命中时覆盖基础描述。消费方：dashboard `GET /api/dashboard/locations`
  的 `game_day` + `resolved_descriptions` 字段（天道全知视角：不过滤隐藏节点，
  但描述随时代演进）。
- Dashboard `export_graph` 不过滤（天道全知视角）。

## 4. 数据画像守卫（CI）

| 守卫                                                  | 位置                                                         | 语义                       |
| ----------------------------------------------------- | ------------------------------------------------------------ | -------------------------- |
| 悬空边端点 / 悬空 parent_id / 重复 node_id / 区间倒置 | `locations_loader::validate_locations`（fail-fast 启动拦截） | 坏图拒绝启动               |
| 全图连通（显式+隐式 BFS）                             | `tests/locations_graph_integrity_test.rs`                    | 无孤儿/孤岛                |
| 单向边画像                                            | 同上（当前画像：无）                                         | 引入断头路必须显式更新清单 |
| travel_cost >= 1                                      | 同上                                                         | 0 成本破坏体力语义         |
| 全图任意两点最短 <= 6 tick                            | 同上（`MAX_PAIRWISE_TRAVEL_BUDGET`）                         | 新增远端地块需重估预算     |
| 出生点存在且全域可达                                  | 同上                                                         | spawn 引用完整性           |
| 非对称边                                              | loader `tracing::warn`                                       | 合法特性，仅提示           |

## 5. 调用关系

```
locations.yaml ──load_locations──> validate_locations（fail-fast）
                                    └──> LocationRegistry（cache.rs）
                                          ├── execute_move（终点可见性 + is_connected）
                                          ├── broadcaster（get_visible_neighbors 邻接过滤）
                                          └── dashboard export_graph（全知不过滤）
time.yaml + game_rules.yaml ──> TimeRegistry::game_day(tick)（时代基准，单一换算源）
```

任何地点相关新需求（多跳寻路、动态节点、势力范围）应复用上述换算与
可见性判定，**禁止**在业务代码里重新硬编码 tick→游戏日公式。

换算真源收敛现状（2026-09-11）：

- 已收编：chronicle `calculate_game_days`、broadcaster `compute_game_time`、
  decay `compute_age_years`、`collector::collect_agents` 周期日、
  chronicle generator `format_tick_range_chinese`、dashboard stats 年月日
  （均委托 `TimeRegistry::try_game_day`/`game_hours`/`game_datetime`，
  等价性有钉死测试）。
- 已知不一致（待统一）：`collect_agents` 的 daily_summary 查询用 0-based
  游戏日，而 `agent_daily_summaries.game_day` 为 1-based——历史行为，
  统一时需迁移查询语义。
