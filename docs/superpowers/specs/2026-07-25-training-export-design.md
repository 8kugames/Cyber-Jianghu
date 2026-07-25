# 训练数据自动导出（Server 端 SFT Export）

**状态**：设计完成，待评审
**日期**：2026-07-25
**作者**：ZCode brainstorming session
**关联**：`docs/DATA_USAGE.md`、`docs/plans/TODO.md:51,113-121`（缺评估回路）、`scripts/build_sft_data.py`、`scripts/build_dpo_data.py`

---

## 1. 目标与背景

### 1.1 现状

- Server 已把 trace 落盘到 `<data_dir>/traces/soul=<stage>/agent=<id>/date=<YYYY-MM-DD>.jsonl`（`crates/server/src/websocket/handler.rs:937`）。
- 落盘时已用 `device_id → agent_id` 反查（`handler.rs:943`），**覆盖客户端传值**，agent_id 关联在落盘时即成立。
- SFT/DPO 导出目前完全是**手动跑离线 Python 脚本**（`scripts/build_sft_data.py`、`scripts/build_dpo_data.py`），无自动机制。
- Server 侧 trace 目录无 LRU 清理（agent 侧有，`trace.rs:340`），会无限增长。
- 无任何 HTTP 导出 endpoint、无 cron、无 job queue。后台任务模板成熟（`init_governance`、`start_telemetry_collector`）。

### 1.2 目标

在 Server 端增加明确的 trace 持久化/导出任务，自动产出**可直接喂训练的 SFT JSONL**，按消息中的真实 `agent_id` 建立关联。

### 1.3 非目标（YAGNI）

- **DPO 偏好对自动化不在本功能范围**——继续走离线 `scripts/build_dpo_data.py`。理由：DPO 配对逻辑复杂且高频迭代，内化到 Rust 维护成本高于收益。
- **server 侧 trace LRU 清理不在本功能范围**——独立议题，本功能不动原始 trace。
- **全量历史重算**——手动触发可指定 `force_full`，但不做"扫描全部历史"的自动行为。

---

## 2. 第一性约束：绝不影响 server 正常运转

### 2.1 项目特质

**agent 24 小时在线**，tick 引擎每 60s 跑一次不可停，WebSocket 连接持续保持。导出任务是**纯附加**的后台任务，其设计铁律：

> 导出宁可慢、宁可丢一次 run，也绝不抢热路径资源。

### 2.2 干扰面矩阵（10 个资源点全覆盖）

基于对 `crates/server/src/` 的彻底审计，所有共享资源点与导出任务的关系：

#### 🔴 高风险——绝不引用

| # | 资源 | 类型 | 位置 | 处理 |
|---|---|---|---|---|
| 1 | `event_manager` | `Arc<std::sync::Mutex>` | `tick/event_manager.rs:28` | 完全不引用。tick 边界高频持有的 std Mutex，碰它就是死锁。 |
| 2 | `rate_limiter` | tokio RwLock | `state.rs:40` | 不引用。每 intent 拿 write lock，导出若碰会阻塞 intent 入队。 |
| 3 | `mpsc worker_tx` (bounded 256) | mpsc channel | `tick/realtime.rs:1165` | 不引用。那是 IntentWorker 的食槽，导出绝不 enqueue 任何 WorkerMessage。 |

#### 🟡 中风险——完全规避

| # | 资源 | 类型 | 位置 | 处理 |
|---|---|---|---|---|
| 4 | `connection_manager` / `agent_to_device_map` | tokio RwLock | `websocket/connection.rs:24,94` | 导出数据源是 trace 文件 + DB，不遍历 WS 连接，完全规避。 |
| 5 | `GameDataCache.data` | `Arc<std::sync::RwLock>` | `game_data/cache.rs:19` | trace 已含 persona_name/persona_description，不查 game_data，规避。 |
| 6 | DB pool | PgPool (max=20, acquire_timeout=5s) | `config.rs:117-127` | 专用低频查询：单次 run ≤5 次 DB 查询，`DISTINCT ON + IN` 批量化（与 Python `:84-92` 同形式）。**statement_timeout 用 `SET LOCAL` 在短事务内设置，绝不用 `SET SESSION`**（PgPool 连接复用，session 级 GUC 会泄漏到热路径连接，见 §5.3.1 sqlx 骨架）。拿不到连接立即放弃本次 run。 |

#### 🟢 低风险——正常使用

| # | 资源 | 类型 | 位置 | 处理 |
|---|---|---|---|---|
| 7 | 热路径磁盘写 (trace/reward JSONL) | 文件 fd | `handler.rs:964`、`reward/*.rs` | 独立目录隔离：产物写 `training_exports/sft/`，与 `traces/`、`reward/` 物理目录不同。`.tmp` + rename 原子化。 |
| 8 | tokio worker pool | runtime (默认 CPU 核数=12) | `main.rs:414` | 协作式让出：transform 循环每 500 条 `tokio::task::yield_now().await`。绝不 `block_in_place`。 |
| 9 | `agent_state_cache` DashMap | DashMap | `state.rs:211` | 不引用。导出数据全来自 trace 文件 + DB。 |
| 10 | `agent_action_logs` 表 | DB 表（高频写） | `005_tick_system.sql:29` | 读不阻塞写（MVCC）。SELECT 与 intent processor 的 INSERT 物理不冲突。走部分索引 `idx_agent_action_logs_soul_cycle` (`:52`)，避开 tick 边界 DB 写入高峰。 |

### 2.3 错误隔离 7 铁律

1. 导出 task 内**绝不 `unwrap`/`expect`/`panic!`**——所有 Result 用 `match` 或 `?`，最外层 `match Err(e) => warn!(...); continue;`
2. **不持有任何热路径锁**——见上方 🔴 红线资源
3. **DB 连接拿不到立即放弃**——`acquire_timeout=5s` 超时返回 Err，本次 run 跳过，下次定时重试
4. **单个 trace 文件解析失败**——跳过该文件，warning 记录，继续处理其他文件
5. **checkpoint 写失败**——跳过 checkpoint 更新，下次重试（幂等，因为产物每次都是新 run 文件）
6. **整个 run 失败**——写 `run=<id>.meta.json` 标记 `status:"failed"` + error，**不影响下次定时触发**
7. **导出 task panic 防护**——spawn 闭包内用 `catch_unwind` 包裹，绝不让 panic 传播到 main select

---

## 3. 架构总览

### 3.1 三种路径的取舍

| 路径 | 描述 | 结论 |
|---|---|---|
| **A（采用）** | 纯 Rust 内化，server 内 spawn 后台任务 | **最小新依赖**：新增 `ulid`（run_id 时序有序）+ `tokio-util`（流式下载的 `ReaderStream`，features=["io"]），无重型框架。与路径 B（需 Python 运行时）对比仍显著更轻。单一二进制部署不变，与 `init_governance` 模式一致。 |
| B | server 内 spawn 子进程调 Python 脚本 | 部署机必须装 Python3+依赖，跨语言错误处理脆弱，与"纯 Rust 二进制"哲学不符 |
| C | 独立 sidecar + cron + Python | 不算"server 端任务"，违背用户原意；cron 跨 OS 差异大 |

**选 A 的核心理由**：用户明确要求"在 Server 端增加任务"；项目当前是纯 Rust 单二进制部署；SFT 转换逻辑简单（filter + map，~150 行 Rust），双源真相的维护成本低于跨语言子进程的脆弱性。

### 3.2 系统拓扑

```
┌─ server (Rust, 单 tokio runtime, multi-thread, 12 workers) ──────────┐
│                                                                       │
│  main.rs spawn 矩阵                                                   │
│   ├─ accept loop (HTTP)              ← 热路径                         │
│   ├─ WebSocket handlers (3 task/连接)← 热路径                         │
│   ├─ start_tick_engine               ← 热路径 (60s interval)          │
│   ├─ IntentWorker                    ← 热路径 (mpsc consumer)         │
│   ├─ init_governance                 ← 冷路径 (1800s, watch shutdown) │
│   ├─ start_telemetry_collector       ← 冷路径 (分钟级)                │
│   ├─ start_db_health_probe           ← 冷路径 (30s)                   │
│   ├─ start_rate_limiter_cleanup      ← 冷路径 (300s)                  │
│   └─ start_training_exporter  ★ 新增 ← 冷路径 (6h, watch shutdown)    │
│                                                                       │
│  HTTP routes (main.rs)                                                │
│   ├─ POST /api/v1/training/export        (write token) ★ 手动触发    │
│   ├─ GET  /api/v1/training/exports       (read token)  ★ 列表        │
│   ├─ GET  /api/v1/training/exports/{id}  (read token)  ★ 元数据      │
│   ├─ GET  /api/v1/training/exports/{id}/download (read) ★ 下载       │
│   ├─ DELETE /api/v1/training/exports/{id} (write token) ★ 删除       │
│   └─ GET  /api/v1/training/checkpoint    (read token)  ★ 调试        │
│                                                                       │
└───────────────────────────────────────────────────────────────────────┘
```

---

## 4. 数据流与转换规则

### 4.1 五步数据流

```
traces/soul=renhun/agent=<UUID>/date=YYYY-MM-DD.jsonl
   │ (每行一个 TraceEntry, agent_id 在落盘时已反查可信)
   ▼
Step 1: 解析 TraceEntry
   │  字段: agent_id, tick_id, attempt, persona_name, persona_description,
   │        user_prompt, response, ok
   ▼
Step 2: 批量查 DB —— 拿每个 (agent_id, tick_id) 的完整 soul_cycle_metadata
   │  SELECT DISTINCT ON (agent_id, tick_id)
   │         agent_id, tick_id, pipe_seq, soul_cycle_metadata
   │  FROM agent_action_logs
   │  WHERE soul_cycle_metadata IS NOT NULL
   │    AND (agent_id, tick_id) IN ($1)   -- $1: 本次扫描到的 (agent_id, tick_id) 对列表
   │  ORDER BY agent_id, tick_id, pipe_seq DESC
   │
   │  索引: 走 idx_agent_action_logs_soul_cycle 部分索引
   │        (005_tick_system.sql:52, WHERE soul_cycle_metadata IS NOT NULL)
   │  索引命中性: 须在 staging 用 EXPLAIN (ANALYZE, BUFFERS) 验证 (见 §11 验收 #7)
   │  SQL 形式与 scripts/build_sft_data.py:84-92 一致 (DISTINCT ON + pipe_seq DESC)
   │
   │  → 内存 HashMap<(agent_id, tick_id), SoulCycleMetadata>
   │    (取最大 pipe_seq 的那条; metadata.cycles: Vec<SoulCycleAttempt>,
   │     每个 cycle 含 attempt + tianhun.result)
   ▼
Step 3: filter —— 按 attempt 精确匹配天魂 approved
   │  对每个 TraceEntry:
   │    1. 先按 Python 规则过滤 ok=false (见 §4.3): trace.ok == false → 跳过
   │    2. lookup (agent_id, tick_id) → 拿到 SoulCycleMetadata
   │    3. 在 metadata.cycles 里找 cycle.attempt == trace.attempt 的那条
   │       (不是 cycles[last]; 是 attempt 精确匹配的那条)
   │    4. 该条 cycle.tianhun.result == "approved" → 保留
   │  → 命中则保留
   ▼
Step 4: transform —— 转成 SFT 样本 (命中一条产一条)
   │  SftSample { messages: [system?, user, assistant], metadata: {...} }
   │  (system 条件 append, 见 §4.2; persona_name 空时 messages 只含 user+assistant)
   ▼
Step 5: 写产物
   training_exports/sft/run=<ULID>.jsonl       (每行一个 SftSample)
   training_exports/sft/run=<ULID>.meta.json   (元数据)
```

### 4.2 persona 拼装规则（与 Python 脚本 `build_sft_data.py:172-183` 对齐）

system message 是**条件 append**（不是强制三消息）。messages 数组始终含 user + assistant，system 仅当 persona_name 非空时 append：

| persona_name | persona_description | messages 构成 | system content（若有） |
|---|---|---|---|
| 有 | 有 | system + user + assistant | `你是 {name}。\n{description}` |
| 有 | 空 | system + user + assistant | `你是 {name}。` |
| 空 | 有 | user + assistant（无 system） | — |
| 空 | 空 | user + assistant（无 system） | — |

**不跳过样本**。persona 双空时仍导出（messages 只含 user+assistant），与 Python `:176-183` 的 `if persona_name:` 条件 append 一致。实测人魂 trace 的 persona_name 来自 `engine.rs:1358 persona.name.clone()`，几乎不会为空；空是边界，对齐 Python 即可，不臆造"跳过"规则。

### 4.3 边界行为

| 边界 | 行为 | 理由（事实依据） |
|---|---|---|
| `trace.ok=false` 或 `response` 为空 | **过滤，跳过该 trace** | 与 Python `build_sft_data.py:163-165` 一致。ok=false 意味着 LLM 调用失败（解析错误/网络错误/空响应），response 是垃圾数据，作为训练数据有害。 |
| DB 查不到 `soul_cycle_metadata` | **跳过该 trace，不报错** | tick 还没跑完写入 `agent_action_logs`；下次增量重试（文件 mtime 会变，触发重扫）。 |
| `metadata.cycles` 为空 | **跳过该 trace** | 无审查记录可关联。 |
| `trace.attempt` 在 `cycles` 中无对应条目 | **跳过该 trace** | 该 attempt 未被审查（数据不一致），不能臆造审查结论。 |
| persona 双空 | **仍导出**（messages 只含 user+assistant） | 与 Python `:176-183` 一致；无 system 的 messages 对训练框架合法，不跳过样本。 |
| `cycles` 中找到 `attempt == trace.attempt` 但 `tianhun.result != "approved"` | **跳过该 trace** | 该 attempt 被天魂驳回，其人魂输出不应作为正向训练样本（否则在教模型生成被驳回的输出）。**这是对 Python 的有意偏离，见 §4.4。** |

### 4.4 与 Python 脚本的契约对照（如实声明对齐与有意偏离）

| 字段/规则 | Python (`build_sft_data.py`) | Rust 版本 | 关系 | 依据 |
|---|---|---|---|---|
| 输入源 | 读 traces 目录 + 连 DB | 同 | ✓ 对齐 | — |
| join key | `(agent_id, tick_id)` | 同 | ✓ 对齐 | — |
| DB 查询 SQL | `DISTINCT ON (agent_id, tick_id) ... ORDER BY pipe_seq DESC` (`:84-92`) | 同 | ✓ 对齐 | 取最大 pipe_seq 的 soul_cycle_metadata |
| ok 过滤 | 过滤 ok=false (`:163-165`) | 过滤 ok=false | ✓ 对齐 | ok=false 是 LLM 失败，response 是垃圾 |
| response 空过滤 | 过滤 (`:163`) | 过滤 | ✓ 对齐 | — |
| persona 拼装 | 条件 append system (`:176-183`) | 条件 append system | ✓ 对齐 | persona_name 空时 messages 无 system role |
| 输出格式 | `{"messages":[...], "metadata":{...}}` | 同 | ✓ 对齐 | vLLM/Axolotl 兼容 |
| **天魂结果 lookup** | `cycles[-1].tianhun.result` (`:107`)，**不区分 trace 属于哪个 attempt** | `cycles.find(attempt == trace.attempt).tianhun.result` | ⚠️ **有意偏离** | 见下方"有意偏离说明" |
| 输出文件组织 | 单一 jsonl（全量） | 按 run 分文件（增量） | ⚠️ 不同 | 增量导出必然 |

**有意偏离说明（天魂结果 lookup）**：

Python `:100-111` 对一个 `(agent_id, tick_id)` 只取 `cycles[-1]`（最后一个 cycle）的 `tianhun.result`，然后**把这个 tick 的所有人魂 trace 都按这个结果筛选**。这意味着：若 attempt 0 被天魂 `rejected`、attempt 1 重写后 `approved`，Python 会把这个 tick 的**两条人魂 trace 都导出**——包括被 `rejected` 的 attempt 0 那条。

Rust 版本改为**按 attempt 精确匹配**：每条人魂 trace（带 `attempt` 字段，`engine.rs:1355` 与 `SoulCycleAttempt.attempt` 同源）只关联 `cycles` 中 `attempt == trace.attempt` 的那条 cycle 的审查结果。只有该 attempt 被 `approved` 才导出。

**偏离的事实依据**（非臆测）：`SoulCycleMetadata.cycles: Vec<SoulCycleAttempt>`（`messages.rs:458`），每个 cycle 独立携带 `tianhun.result`。把被驳回的 attempt 作为正向 SFT 样本，等于教模型生成天魂不认可的输出——这是训练数据污染。Python 的处理是该污染源；Rust 版本修正它。

**双源真相同步机制（修订）**：

由于 Rust 版本对天魂 lookup 有意偏离 Python，黄金对照测试**不能**用 Python 现状产出黄金集（会包含被污染的样本）。机制改为：

1. **基准集**：用 Python `--no-db-filter` 模式（跳过天魂筛选，`:251-253`）产出"所有人魂 trace 转 SFT 样本"的基准集。此模式下 Python 不做天魂 lookup，与 Rust 的天魂偏离无关，可作为 transform 纯函数（persona 拼装 + ok 过滤 + response 空过滤）的对照基准。
2. **天魂筛选的独立测试**：Rust 的 attempt 精确匹配逻辑用**构造的 fixture**（手写 cycles 数组含多 attempt 的 approved/rejected 混合）验证，不走 Python 对照。fixture 覆盖：单 attempt approved、单 attempt rejected、多 attempt 混合、attempt 缺失。
3. **任何 transform 规则改动**（persona/ok/response）必须先改 Python 产出新基准集，再改 Rust 通过测试。
4. **任何天魂 lookup 改动**只改 Rust fixture，不动 Python（Python 的天魂逻辑是已知 bug，不作为黄金标准）。

---

## 5. 参数推导（第一性原理 + 实测数据）

### 5.1 实测基线

| 维度 | 实测值 | 来源 |
|---|---|---|
| 1 tick | 60s | `game_rules.yaml:38` |
| trace 产生率 | 2~4 条/agent/tick | `engine.rs:1349` + `validator.rs:616` |
| 单条 trace | ~3.6 KB | `crates/server/data/traces/soul=renhun` 实测 32,656 行 / 117MB |
| 100 agent 满载 | 200~400 条/tick ≈ 3.3~6.7 条/s ≈ 1.7 GB/天 | 推导 |
| agent 上限 | 无代码 cap，实测历史峰值 24 个 | `.test-agents/` + 24 distinct 目录 |
| PgPool | max=20, acquire_timeout=5s, statement_timeout=0(无限) | `config.rs:117-127` |
| 磁盘 | APFS 4KB 块，525GB 可用 | `diskutil` |
| 保留策略 | LRU 1024MB（仅 agent 侧），server 侧无 LRU | `DATA_USAGE.md:105-109` |
| DB join 锚点 | `(agent_id, tick_id)` 走部分索引 | `005_tick_system.sql:52` |

### 5.2 参数汇总表（每个参数都有物理极限推导 + 实测约束 + 权衡空间）

| 参数 | 推荐值 | 物理极限 | 裕量 | 权衡空间 |
|---|---|---|---|---|
| 定时间隔 | **6h** | 60s (1 tick) | 360× | 1h-24h；信息论视角：样本价值不随时间衰减，新鲜度非核心价值。系统论视角：单位数据干扰成本 ∝ √频率，最小化频率但受 LRU 约束。6h 是干扰/新鲜度最优点。 |
| 单次 run trace 上限 | **50,000 条** | ~1M（内存 18GB / 3.6KB） | 20× | 10k-100k；实测 6h 累积最多 18k 条，50k 是 2.7× 裕量，覆盖间隔调长到 12h 或 agent 规模峰值到 200。 |
| run 超时 | **10 min** | 最坏 ~162s（推导） | ~3.7× | 推导：50k 条纯 CPU ~0.5s + yield 让出最坏 +10s + DB 查询最坏 5批×30s=150s + 文件 IO ~2s = **162.5s**。10 min (600s) / 162.5s ≈ **3.7×** 裕量。覆盖 IO 抖动 + 调度延迟。（v1 误算为 35s/17×，v2 修订误算为 153s/3.9×——两次都加错：0.5+10+150+2=162.5。） |
| DB 批大小 | **10,000 对** | 65,535（PG 绑定参数 UINT16 上限，[PG 官方 limits](https://www.postgresql.org/docs/current/limits.html)） | 6.5× | 1k-10k 几乎等价（索引高效区）；选 10k 减少 DB 往返次数。注：本设计用 `UNNEST($1::uuid[], $2::bigint[])` 只绑 2 个数组参数，不受 65535 约束，但批大小仍控在 10k 以控内存 + 索引扫描成本。 |
| DB 连接占用 | **1** | 11（pool 余量 20×0.9-7） | 11× | 1-3；串行 task 物理上只需 1 连接，是严格最优解，占用 pool 5%。 |
| 产物大小上限 | **50 GB** | 525 GB（磁盘） | 10× | 25-100 GB；产物 ≈ trace × 0.5（approved 率）≈ 0.85 GB/天，50GB 覆盖 ~59 天训练迭代周期。 |
| 让出频率 | **每 500 条** | 内存局部性 | — | 100-1000 几乎等价（让出开销 ≤1%）；500 是中间值，50k 条 run 总让出开销 ~1ms。 |
| checkpoint 粒度 | **trace_id 集合**（幂等去重） | 行级偏移 | — | 第一性分析三选项：(a) 行级偏移——崩溃恢复复杂、易空洞；(b) 下游去重——产物 4× 膨胀；(c) **trace_id 集合（采用）**——trace_id 全局唯一（UUID），checkpoint 记录已处理的 trace_id，下次读整个文件但跳过已处理。幂等、崩溃安全（没写产物就不更新集合）、无空洞。内存：50k 条 × 36B UUID = 1.8MB 可接受。修订前误用文件级 (size,mtime)，在 append-only 当日文件上会跨 run 重复发出同一 SftSample。 |

**所有参数都有物理极限推导 + 实测约束 + 明确权衡空间标注。没有任何参数是拍脑袋取值。**

### 5.2.1 checkpoint 退役策略（防无限膨胀）

trace_id 集合会随历史累积膨胀（Architecture Auditor A-05：跨 30 天可达 216MB）。退役依据：trace 文件按日期分区（`date=YYYY-MM-DD.jsonl`），老日期文件不再增长，其 trace_id 永远不会被再次遇到，是死重量。

**退役机制**：checkpoint 按**日期分桶**记录，key 为 `date=YYYY-MM-DD`，value 为该日期所有 trace_id 的集合。每次 run 结束后，扫描 checkpoint 中**超过 N 天**（默认 N=7，对齐 trace 文件保留周期）的日期桶，整桶删除。

- 单桶上限：50k 条 × 36B = 1.8MB（单次 run）
- 保留 N=7 天：7 桶 × 1.8MB = 12.6MB（有界，不再无限膨胀）
- 安全性：被删桶的 trace_id 对应的 trace 文件也已老化（同一周期），即便重扫也不会遇到（文件已被 agent 端 LRU 或未来 server 端清理移除）。若 trace 文件仍在但 checkpoint 桶被删，最坏情况是该日期文件被重新处理——产物写到新 run 文件，下游按 trace_id 去重，幂等无空洞。

### 5.3 资源用量硬上限（超限即停止本次 run）

| 维度 | 硬上限 | 触发后行为 |
|---|---|---|
| 单次 run trace 文件数 | ≤ 500 | 按 mtime 排序处理最老的 500 个，其余下次处理 |
| 单次 run trace 总条数 | ≤ 50,000 | 停止本次 run，下次增量继续 |
| 单次 run DB 查询次数 | ≤ 5 | 用 `DISTINCT ON + IN` 批量化（与 Python `:84-92` 同形式） |
| 单次 run 时长 | ≤ 10 分钟 | `tokio::time::timeout` 强制中止 |
| 单次 DB 查询时长 | ≤ 30s | **`SET LOCAL statement_timeout` 在短事务内**（绝不用 SET SESSION，防 GUC 泄漏到热路径连接）。sqlx 骨架见 §5.3.1。 |
| DB 连接占用 | ≤ 1 | 同时只拿 1 个连接，用完即释放 |
| 产物目录总大小 | ≤ 50 GB | 停止新 run，发 warning，等手动清理 |
| 并发 run | ≤ 1 | `AtomicBool` 互斥，上次还在跑则跳过本次触发 |

### 5.3.1 DB 查询的事务包裹（防 GUC 泄漏）

**问题**（Integration Auditor IA-02）：`crates/server/src/db/common.rs:98-108` 的 PgPool `test_before_acquire(true)` 意味着连接在多次 acquire 间复用。若用 `SET SESSION statement_timeout = '30s'`，这个 GUC 会残留在连接上，下一个 acquire 该连接的热路径 SQL（intent INSERT、tick_logs INSERT）一旦超过 30s 就被 PostgreSQL 主动 abort。这是热路径污染。

**解决**：用短事务 + `SET LOCAL`。`SET LOCAL` 的 GUC 生命周期仅限当前事务，`COMMIT`/`ROLLBACK` 后自动恢复（PG 服务端行为，与 `test_before_acquire` 无关）。sqlx 骨架：

```rust
// runner.rs —— 单次 DB 查询的包裹模式
async fn fetch_soul_cycle_metadata(
    pool: &PgPool,
    keys: &[(Uuid, i64)],  // (agent_id, tick_id) 对列表
) -> Result<HashMap<(Uuid, i64), SoulCycleMetadata>, ExportError> {
    // 关键：开短事务（纯 SELECT，不写表），SET LOCAL 在事务内，commit 后 GUC 自动清除。
    // 注意：不用 SET LOCAL default_transaction_read_only —— 该 GUC 只影响"未来"事务的默认值，
    // 对已 BEGIN 的当前事务语义为空（Integration Auditor IA-V02-N1）。本查询只 SELECT，无需强制只读。
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL statement_timeout = '30s'")
        .execute(&mut *tx).await?;

    // 与 build_sft_data.py:84-92 同形式
    let rows = sqlx::query_as::<_, SoulCycleRow>(
        r#"
        SELECT DISTINCT ON (agent_id, tick_id)
               agent_id, tick_id, soul_cycle_metadata
        FROM agent_action_logs
        WHERE soul_cycle_metadata IS NOT NULL
          AND (agent_id, tick_id) IN (SELECT * FROM UNNEST($1::uuid[], $2::bigint[]))
        ORDER BY agent_id, tick_id, pipe_seq DESC
        "#,
    )
    .bind(&keys.iter().map(|(a, _)| *a).collect::<Vec<_>>())  // $1: uuid[]
    .bind(&keys.iter().map(|(_, t)| *t).collect::<Vec<_>>())  // $2: bigint[]
    .fetch_all(&mut *tx).await?;

    tx.commit().await?;  // SET LOCAL 在此自动失效，连接归还 pool 时 GUC 已清除
    Ok(rows.into_iter().map(|r| ((r.agent_id, r.tick_id), r.metadata)).collect())
}
```

**IN 子句用 `UNNEST($1::uuid[], $2::bigint[])` 而非复合类型**：避免复合类型 `(uuid, bigint)` 在 sqlx 的 Type derive 复杂性，且 PostgreSQL 对 `UNNEST` 配对的等值谓词能正常下推到 B-tree 索引（不像复合类型 ANY 可能退化为 seq scan）。这是对 IA-01 的预防性规避。

**验收**（§11 增 #8）：在 staging 连续跑导出查询后，立即查 `SHOW statement_timeout;`（在新 acquire 的连接上），确认返回 `0`（reset），证明 GUC 未泄漏。

---

## 6. HTTP API 设计

### 6.1 端点清单

鉴权函数对齐现有 `crates/server/src/handlers/auth.rs` 三档：
- "write token" = `require_write_token`（admin 写操作）
- "read token" = `require_client_read_token`（与 dashboard 同档，便于工具拉取）

| 方法 | 路径 | 鉴权 | 作用 |
|---|---|---|---|
| `POST` | `/api/v1/training/export` | `require_write_token` | 手动触发一次 run（可指定 agent_id/全量） |
| `GET` | `/api/v1/training/exports` | `require_client_read_token` | 列出所有 run 元数据（ULID 游标分页） |
| `GET` | `/api/v1/training/exports/{run_id}` | `require_client_read_token` | 单个 run 元数据 |
| `GET` | `/api/v1/training/exports/{run_id}/download` | `require_client_read_token` | 流式下载产物 JSONL |
| `DELETE` | `/api/v1/training/exports/{run_id}` | `require_write_token` | 删除一个 run（含文件） |
| `GET` | `/api/v1/training/checkpoint` | `require_client_read_token` | 查看增量 checkpoint 状态（调试用） |

### 6.2 请求/响应 schema

**POST /api/v1/training/export**
```json
// Request body (全可选)
{
  "agent_id": "00000000-0000-0000-0000-000000000000",
  "force_full": false
}
// Response 202 Accepted
{
  "run_id": "01J9...",
  "status": "pending",
  "triggered_by": "manual",
  "started_at": "2026-07-25T..."
}
```

**GET /api/v1/training/exports**
```json
{
  "runs": [
    {
      "run_id": "01J9...",
      "status": "completed",
      "triggered_by": "scheduled",
      "started_at": "...",
      "completed_at": "...",
      "agent_id": null,
      "trace_count": 18234,
      "sample_count": 9127,
      "output_path": "training_exports/sft/run=01J9....jsonl",
      "output_size_bytes": 33554432,
      "error": null
    }
  ],
  "total": 42,
  "next_cursor": "01J9..."
}
```

**GET /api/v1/training/exports/{run_id}/download**
- `200 OK` + `Content-Type: application/x-jsonlines` + `Content-Disposition: attachment` + 流式 body（`tokio_util::io::ReaderStream` + `Body::from_stream`）
- `404` if run_id 不存在或产物文件已删除

### 6.3 设计决策

- **ULID 而非 UUID**：run_id 需时序有序（游标分页），ULID 前 48bit 毫秒时间戳 + 80bit 随机，字典序 = 时间序，`ls` 即可看出顺序，与 UUID v4 兼容（128-bit）。新增依赖 `ulid` crate（纯 Rust 无 unsafe）。
- **不写 DB 表记录 run 历史**：run 元数据是冷数据，文件系统（APFS）已是可靠存储，写 `.meta.json` 旁路文件即可。列表 endpoint 扫描目录读所有 `.meta.json`，实测 run 量级（4 次/天 × 30 天 = 120 个）完全可控。**权衡空间**：若未来 run 数 > 10,000（2 年以上），目录扫描变慢，可加 DB 表；当前 YAGNI。

---

## 7. 配置设计

### 7.1 配置文件

新增 `crates/server/config/training_export.yaml`（与 `game_rules.yaml`、`network.yaml` 同级）：

```yaml
# 训练数据自动导出配置
# 参数推导见 docs/superpowers/specs/2026-07-25-training-export-design.md §5
enabled: false                    # 默认关闭, 显式开启

scheduler:
  interval_secs: 21600            # 6h
  run_timeout_secs: 600           # 10min
  max_concurrent_runs: 1          # 严格 1, 串行

limits:
  max_traces_per_run: 50000
  max_total_export_size_gb: 50
  db_batch_size: 10000
  db_statement_timeout_secs: 30
  yield_every_n: 500

paths:
  traces_input_subdir: "traces/soul=renhun"
  output_subdir: "training_exports/sft"
  checkpoint_filename: "training_exports/sft_checkpoint.json"
```

### 7.2 环境变量覆盖（运行时调参）

**加载模式**：复刻 `main.rs:258-268` 的 action_evolution.yaml 内嵌加载模式（`std::fs::read_to_string` + `serde_yaml::from_str` + env 覆盖），**不走** `Config` struct（`config.rs` 的 `Config` 只含 server/database 两段，无 yaml 加载层；DB 配置走 `config.rs:184-210` 的纯 `std::env::var` 逐字段读取）。加载优先级：**env > training_export.yaml > 代码内 default**。

**环境变量命名**：对齐项目现有扁平命名规范（`SERVER_HOST` / `DB_MAX_CONNECTIONS` / `ADMIN_WRITE_TOKEN`，见 `config.rs:166-219`；只有路径类用 `CYBER_JIANGHU_` 前缀，见 `paths.rs`）。因此用扁平 `TRAINING_EXPORT_*`，**不**引入 `CYBER_JIANGHU_TRAINING_EXPORT_*` 新前缀层级：

| 环境变量 | 默认 |
|---|---|
| `TRAINING_EXPORT_ENABLED` | `false` |
| `TRAINING_EXPORT_INTERVAL_SECS` | `21600` |
| `TRAINING_EXPORT_RUN_TIMEOUT_SECS` | `600` |
| `TRAINING_EXPORT_MAX_TRACES` | `50000` |
| `TRAINING_EXPORT_MAX_SIZE_GB` | `50` |
| `TRAINING_EXPORT_DB_BATCH` | `10000` |

### 7.3 加载时机

在 `main.rs` 启动序列、`AppState` 构建之前加载（对齐 `game_rules` 加载时机，`main.rs:483` 的 `game_data::init_registry` 之后）。**`enabled: false` 时不 spawn，零开销。**

---

## 8. shutdown 协议

### 8.1 设计原则

优雅关闭的充要条件：
1. 收到 shutdown 信号后，正在运行的 run 能在有限时间内完成或中止
2. checkpoint 状态不丢失（下次启动能续上）
3. 不产生半成品产物文件（避免下次启动读到损坏 JSONL）

### 8.2 实现（复刻 `init_governance` 模式）

**复刻要点**（对齐 `main.rs:289-330` 的 init_governance 真实实现）：init_governance 不是只用外层 5s join——它在**每个 interval tick body 内**用 `tokio::time::timeout(review_timeout, ...)` 包裹单次轮询（`main.rs:310, 322`），这样 shutdown 信号到来时，正在进行的单次操作能在 `review_timeout` 内结束（而非等到下次 tick）。training_exporter 必须复刻这个**双层 timeout**：
- 外层：`tokio::time::timeout(5s, handle)` join（main 关闭序列，`main.rs:1310-1316` 模式）
- 内层：`tokio::time::timeout(run_timeout, run_once(...))` 包裹单次 run（scheduler task 内，每个 tick body）

scheduler task 内部循环结构：
```rust
loop {
    tokio::select! {
        _ = shutdown_rx.changed() => {
            if *shutdown_rx.borrow() { break; }
        }
        _ = interval.tick() => {
            if is_running.swap(true, SeqCst) { warn!("上次 run 仍在进行, 跳过"); continue; }
            // 内层 timeout: 单次 run 受 run_timeout (默认 600s) 约束
            // shutdown 信号到来时, 这里最多等 run_timeout 而非无限等
            let run_result = tokio::time::timeout(
                Duration::from_secs(config.run_timeout_secs),
                run_once(&config, &db_pool, &checkpoint, TriggerSource::Scheduled),
            ).await;
            is_running.store(false, SeqCst);
            match run_result {
                Ok(Ok(meta)) => info!(run_id=%meta.run_id, "run 完成"),
                Ok(Err(e)) => warn!(?e, "run 失败"),
                Err(_elapsed) => warn!("run 超时 (>{:?}), 本次中止", config.run_timeout_secs),
            }
        }
    }
}
```

```rust
// main.rs 启动序列
let (export_shutdown_tx, export_shutdown_rx) = tokio::sync::watch::channel(false);
// 独立 watch channel, 不与 governance shutdown 共用 (隔离故障域)

let exporter_handle = if config.enabled {
    Some(tokio::spawn(start_training_exporter(
        config, app_state.db_pool.clone(), export_shutdown_rx,
        app_state.training_export_manual_trigger.clone(),
    )))
} else { None };

// 关闭序列
let _ = export_shutdown_tx.send(true);
if let Some(handle) = exporter_handle {
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
}
```

### 8.3 半成品产物防护

- 写 `.run=<id>.jsonl.tmp`
- 写完 + **fsync（破例一次，shutdown 崩溃概率高）**
- rename 到正式名
- shutdown 中断残留的 `.tmp` 文件 → 下次启动 `start_training_exporter` 启动时先 sweep 清理
- checkpoint 写：`.checkpoint.json.tmp` + rename，同模式

### 8.4 手动触发与定时任务的互斥

手动 POST 走 mpsc 发给**同一个 scheduler task**（不单独 spawn），由 scheduler 统一调度，保证 `max_concurrent_runs=1` 互斥。手动触发到达时若定时 run 正在跑，按 §10 待决问题 #1 的推荐方案处理：manual request 排队 + 30s 超时返回 408 Request Timeout。

---

## 9. 模块结构

```
crates/server/src/training_export/
├── mod.rs                 # 公共类型 (RunMetadata, RunStatus, TriggerSource)
├── config.rs              # TrainingExportConfig 加载 (env > yaml > default)
├── checkpoint.rs          # Checkpoint 读写 (trace_id 集合幂等去重, §5.2)
├── sft_transform.rs       # 纯函数: transform(entries, audit_map) -> Vec<SftSample>
├── runner.rs              # 单次 run 编排 (扫文件 → DB 查询 → transform → 写产物)
├── scheduler.rs           # 后台 task: 启动时 sweep *.tmp 残留 (§8.3) + interval + 双层 timeout + shutdown + 手动触发 mpsc
└── handlers.rs            # HTTP handlers 内部逻辑

crates/server/src/handlers/
└── training_export.rs     # axum handlers (薄封装, 调 training_export::handlers)

crates/server/config/
└── training_export.yaml   # 默认配置

crates/server/tests/
└── training_export_sft_transform.rs  # 黄金对照测试

crates/server/tests/sft_golden/
└── *.jsonl                # Python 产出的黄金集 (commit 进仓)
```

### 9.1 模块依赖（单向无环）

```
handlers.rs ──> scheduler.rs ──> runner.rs ──> sft_transform.rs (纯函数, 无 IO)
                    │                  │
                    │                  ├──> checkpoint.rs
                    │                  └──> db (agent_action_logs 查询)
                    │
                    └──> config.rs
mod.rs (公共类型, 被所有人依赖)
```

**关键约束**：`sft_transform.rs` 是纯函数模块，无 IO、无 DB、无全局状态，可独立单元测试。

### 9.2 核心数据结构

```rust
// training_export/mod.rs
pub enum TriggerSource { Scheduled, Manual }
pub enum RunStatus { Pending, Running, Completed, Failed, Skipped }

pub struct RunMetadata {
    pub run_id: String,                  // ULID
    pub status: RunStatus,
    pub triggered_by: TriggerSource,
    pub started_at: i64,                 // Unix ms
    pub completed_at: Option<i64>,
    pub agent_id_filter: Option<Uuid>,   // None=所有 agent
    pub force_full: bool,
    pub trace_count: usize,
    pub sample_count: usize,
    pub output_path: String,
    pub output_size_bytes: u64,
    pub error: Option<String>,
    pub schema_version: u32,
}

// sft_transform.rs (与 Python 输出严格一致)
pub struct SftSample {
    pub messages: Vec<SftMessage>,
    pub metadata: SftSampleMetadata,
}
pub struct SftMessage { pub role: String, pub content: String }
pub struct SftSampleMetadata {
    pub agent_id: String, pub tick_id: i64, pub soul_stage: String,
    pub attempt: i32, pub provider: String, pub model: String,
    pub tianhun_result: String, pub trace_id: String,
}
```

---

## 10. 待决问题（实现时定）

| # | 问题 | 候选 | 推荐 |
|---|---|---|---|
| 1 | 手动触发时定时 run 正在跑 | (a) 排队等待 (b) 返回 409 Conflict (c) 排队但有超时 | (c) 排队 + 30s 超时返回 408 |
| ~~2~~ | ~~复合类型数组 sqlx 映射~~ | **已解决**（§5.3.1 改用 `UNNEST($1::uuid[], $2::bigint[])`，避免复合类型，索引命中性见 §11 验收 #7） | — |
| 3 | 流式下载是否支持 Range 请求（断点续传） | (a) 不支持，整体下载 (b) 支持 Range | (a) YAGNI，产物单文件最大 ~180MB |
| 4 | 是否提供"按 agent_id 筛选下载" | (a) 不提供，下载整个 run (b) 提供 per-agent 文件 | (a) YAGNI，run 已含 metadata.agent_id，下游可自行筛 |

---

## 11. 验收标准

1. **transform 纯函数正确性**：`sft_transform.rs` 用 Python `--no-db-filter` 模式产出的基准集（`crates/server/tests/sft_golden/`）做黄金对照，persona 拼装 + ok 过滤 + response 空过滤三规则与 Python `build_sft_data.py:163-183` 一致。天魂筛选逻辑用独立 fixture 验证（因有意偏离 Python，见 §4.4）。
2. **零热路径影响**：导出运行期间，tick 引擎延迟不增加（benchmark：导出 run 中 tick 延迟 vs 空闲 tick 延迟，差值 < 5%）
3. **优雅关闭**：SIGINT 后 5s 内 exporter task 退出（双层 timeout：外层 5s join + 内层 run_timeout），无 `.tmp` 残留（或残留被下次启动 sweep）
4. **错误隔离**：构造 DB 连接耗尽场景，导出 run 跳过，server 主流程不受影响
5. **增量正确性（trace_id 幂等）**：连续两次定时 run 处理同一 append-only 当日文件，第二次不重复发出第一次已处理的 SftSample（按 trace_id 去重，见 §5.2）
6. **配置开关**：`enabled: false` 时零 spawn、零开销（启动日志确认）
7. **DB 索引命中性**（staging 验证）：对 §5.3.1 的 SQL 跑 `EXPLAIN (ANALYZE, BUFFERS)`，确认走 `idx_agent_action_logs_soul_cycle` 部分索引（Index Scan / Bitmap Index Scan），而非 Seq Scan。若 Seq Scan 则触发 §5.3.1 备选方案评估。
8. **GUC 未泄漏**（staging 验证）：连续跑导出查询后，在新 acquire 的 PgPool 连接上执行 `SHOW statement_timeout;`，确认返回 `0`（reset），证明 `SET LOCAL` 未泄漏到热路径连接。

---

## 12. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| 双源真相（Python vs Rust）漂移 | 中 | 训练数据质量 | 黄金对照测试 + CI 强制 |
| 复合类型 sqlx 映射不熟 | — | — | **已消除**（§5.3.1 改用 `UNNEST($1::uuid[], $2::bigint[])`，只绑 2 个数组参数，无需自定义 PgComposite） |
| server 侧 trace 无限增长 | 已知 | 磁盘满 | 本功能不动，独立议题；配置 `max_total_export_size_gb` 限制产物侧 |
| ULID 时钟回拨 | 极低 | run_id 乱序 | ULID 库内置单调保护；乱序只影响列表顺序，不影响正确性 |
| 手动触发与定时竞争 | 低 | run 重复 | `AtomicBool` 互斥 + mpsc 统一调度 |

---

## 13. 实施顺序（writing-plans 阶段细化）

1. 新增 `training_export/` 模块骨架 + 公共类型
2. `sft_transform.rs` 纯函数 + 黄金对照测试（TDD）
3. `checkpoint.rs` trace_id 集合增量（幂等去重）
4. `runner.rs` 单次 run 编排（扫文件 + DB 查询 + transform + 写产物）
5. `scheduler.rs` 后台 task（interval + shutdown + mpsc）
6. `config.rs` 加载（env > yaml > default）
7. HTTP handlers + 路由注册
8. `main.rs` spawn 集成 + shutdown 集成
9. 集成测试（零热路径影响 benchmark、优雅关闭、错误隔离）
