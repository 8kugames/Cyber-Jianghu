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
| 6 | DB pool | PgPool (max=20, acquire_timeout=5s) | `config.rs:117-127` | 专用低频查询：单次 run ≤5 次 DB 查询，`ANY(复合类型数组)` 批量化，`statement_timeout=30s` session 级，拿不到连接立即放弃本次 run。 |

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
| **A（采用）** | 纯 Rust 内化，server 内 spawn 后台任务 | 零新依赖（已有 axum+sqlx+tokio+serde_json），单一二进制部署不变，与 `init_governance` 模式一致 |
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
Step 2: 批量查 DB —— 拿天魂审查结果
   │  SELECT agent_id, tick_id, soul_cycle_metadata
   │  FROM agent_action_logs
   │  WHERE ((agent_id, tick_id) = ANY($1::复合类型数组))
   │    AND soul_cycle_metadata IS NOT NULL
   │  走部分索引 idx_agent_action_logs_soul_cycle (005_tick_system.sql:52)
   │  注: sqlx 复合类型映射的具体实现见 §10 待决问题 #2
   │  → 内存 HashMap<(agent_id, tick_id), Vec<CycleResult>>
   ▼
Step 3: filter —— 只保留天魂 approved 的 attempt
   │  对每个 TraceEntry:
   │    lookup (agent_id, tick_id)
   │    找 cycles[last].attempt == trace.attempt
   │    且 cycles[last].tianhun.result == "approved"
   │  → 命中则保留
   ▼
Step 4: transform —— 转成 SFT 样本 (1:1, 命中一条产一条)
   │  SftSample { messages: [system, user, assistant], metadata: {...} }
   ▼
Step 5: 写产物
   training_exports/sft/run=<ULID>.jsonl       (每行一个 SftSample)
   training_exports/sft/run=<ULID>.meta.json   (元数据)
```

### 4.2 persona 拼装规则（与 Python 脚本对齐）

| persona_name | persona_description | 拼出的 system content |
|---|---|---|
| 有 | 有 | `你是 {name}。{description}` |
| 有 | 空 | `你是 {name}。` |
| 空 | 有 | `{description}` |
| 空 | 空 | **跳过该样本**（system 内容为空无法训练） |

### 4.3 边界行为（与 Python 脚本 1:1 对齐）

| 边界 | 行为 | 理由 |
|---|---|---|
| `trace.ok=false` | **不过滤，保留** | 与 Python 脚本一致；靠天魂 approved 过滤，不二次过滤 ok |
| DB 查不到 `soul_cycle_metadata` | **跳过该 trace，不报错** | tick 还没跑完写入；下次增量重试（文件 mtime 会变，触发重扫） |
| persona 双空 | **跳过该样本** | system 内容为空无法训练 |
| attempt 不匹配 cycles[last] | **跳过该 trace** | 非 last attempt 不算审查结论 |

### 4.4 与 Python 脚本的契约对照

| 字段/规则 | Python (`build_sft_data.py`) | Rust 版本 | 一致性 |
|---|---|---|---|
| 输入 | 读 traces 目录 + 连 DB | 同 | ✓ |
| join key | `(agent_id, tick_id)` | 同 | ✓ |
| 审查字段路径 | `soul_cycle_metadata.cycles[last].tianhun.result` | 同 | ✓ |
| approved 过滤 | `== "approved"` | 同 | ✓ |
| ok 过滤 | 不过滤 | 不过滤 | ✓ |
| system 拼装 | `你是 {name}。{description}` | 同（空字段降级） | ✓ |
| 输出 | `{"messages":[{role,content}], "metadata":{...}}` | 同 | ✓ |
| 输出文件 | 单一 jsonl（全量） | 按 run 分文件（增量） | ⚠️ 不同（增量必然） |

**双源真相同步机制**：`sft_transform.rs` 单元测试用 `scripts/build_sft_data.py` 产出的黄金集（commit 进 `crates/server/tests/sft_golden/`），任何规则改动必须先改 Python 产出新黄金集，再改 Rust 通过测试。

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
| run 超时 | **10 min** | ~35s（最坏推导） | 17× | 2-10 min；50k 条纯 CPU ~0.5s，yield 让出最坏 +10s，DB 查询最坏 +25s，总 ~35s。10 min 是 17× 裕量，覆盖 IO 抖动 + 调度延迟。 |
| DB 批大小 | **10,000 对** | 32,767（PG 参数数组 INT16） | 3.3× | 1k-10k 几乎等价（索引高效区）；选 10k 减少 DB 往返次数。 |
| DB 连接占用 | **1** | 11（pool 余量 20×0.9-7） | 11× | 1-3；串行 task 物理上只需 1 连接，是严格最优解，占用 pool 5%。 |
| 产物大小上限 | **50 GB** | 525 GB（磁盘） | 10× | 25-100 GB；产物 ≈ trace × 0.5（approved 率）≈ 0.85 GB/天，50GB 覆盖 ~59 天训练迭代周期。 |
| 让出频率 | **每 500 条** | 内存局部性 | — | 100-1000 几乎等价（让出开销 ≤1%）；500 是中间值，50k 条 run 总让出开销 ~1ms。 |
| checkpoint 粒度 | **文件级 (size, mtime)** | 行级偏移 | — | 文件级 YAGNI 最优；trace 文件每天滚动，最大单文件 6.8MB，无需行级偏移。 |

**所有参数都有物理极限推导 + 实测约束 + 明确权衡空间标注。没有任何参数是拍脑袋取值。**

### 5.3 资源用量硬上限（超限即停止本次 run）

| 维度 | 硬上限 | 触发后行为 |
|---|---|---|
| 单次 run trace 文件数 | ≤ 500 | 按 mtime 排序处理最老的 500 个，其余下次处理 |
| 单次 run trace 总条数 | ≤ 50,000 | 停止本次 run，下次增量继续 |
| 单次 run DB 查询次数 | ≤ 5 | 用 `ANY(复合类型数组)` 批量化 |
| 单次 run 时长 | ≤ 10 分钟 | `tokio::time::timeout` 强制中止 |
| 单次 DB 查询时长 | ≤ 30s | `statement_timeout` session 级 |
| DB 连接占用 | ≤ 1 | 同时只拿 1 个连接，用完即释放 |
| 产物目录总大小 | ≤ 50 GB | 停止新 run，发 warning，等手动清理 |
| 并发 run | ≤ 1 | `AtomicBool` 互斥，上次还在跑则跳过本次触发 |

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

加载优先级：**env > config.yaml > default**（对齐 `config.rs:184-210`）。

| 环境变量 | 默认 |
|---|---|
| `CYBER_JIANGHU_TRAINING_EXPORT_ENABLED` | `false` |
| `CYBER_JIANGHU_TRAINING_EXPORT_INTERVAL_SECS` | `21600` |
| `CYBER_JIANGHU_TRAINING_EXPORT_RUN_TIMEOUT_SECS` | `600` |
| `CYBER_JIANGHU_TRAINING_EXPORT_MAX_TRACES` | `50000` |
| `CYBER_JIANGHU_TRAINING_EXPORT_MAX_SIZE_GB` | `50` |
| `CYBER_JIANGHU_TRAINING_EXPORT_DB_BATCH` | `10000` |

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
├── checkpoint.rs          # Checkpoint 读写 (文件级 size+mtime)
├── sft_transform.rs       # 纯函数: transform(entries, audit_map) -> Vec<SftSample>
├── runner.rs              # 单次 run 编排 (扫文件 → DB 查询 → transform → 写产物)
├── scheduler.rs           # 后台 task (interval + shutdown + 手动触发 mpsc)
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
| 2 | 复合类型数组 `(agent_id, tick_id) = ANY($1)` 在 sqlx 的类型映射 | (a) 自定义 PgComposite + Type derive (b) 拆两条 ANY 用笛卡尔 + 内存精确 match | (a) 复合类型，更准 |
| 3 | 流式下载是否支持 Range 请求（断点续传） | (a) 不支持，整体下载 (b) 支持 Range | (a) YAGNI，产物单文件最大 ~180MB |
| 4 | 是否提供"按 agent_id 筛选下载" | (a) 不提供，下载整个 run (b) 提供 per-agent 文件 | (a) YAGNI，run 已含 metadata.agent_id，下游可自行筛 |

---

## 11. 验收标准

1. **功能正确性**：`sft_transform.rs` 黄金对照测试通过（与 `scripts/build_sft_data.py` 在相同输入下产出相同 SftSample 集合）
2. **零热路径影响**：导出运行期间，tick 引擎延迟不增加（benchmark：导出 run 中 tick 延迟 vs 空闲 tick 延迟，差值 < 5%）
3. **优雅关闭**：SIGINT 后 5s 内 exporter task 退出，无 `.tmp` 残留（或残留被下次启动 sweep）
4. **错误隔离**：构造 DB 连接耗尽场景，导出 run 跳过，server 主流程不受影响
5. **增量正确性**：连续两次定时 run，第二次不重复处理第一次已处理的文件（checkpoint 验证）
6. **配置开关**：`enabled: false` 时零 spawn、零开销（启动日志确认）

---

## 12. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| 双源真相（Python vs Rust）漂移 | 中 | 训练数据质量 | 黄金对照测试 + CI 强制 |
| 复合类型 sqlx 映射不熟 | 中 | 实现延期 | 待决问题 #2 有备选方案（笛卡尔+内存 match） |
| server 侧 trace 无限增长 | 已知 | 磁盘满 | 本功能不动，独立议题；配置 `max_total_export_size_gb` 限制产物侧 |
| ULID 时钟回拨 | 极低 | run_id 乱序 | ULID 库内置单调保护；乱序只影响列表顺序，不影响正确性 |
| 手动触发与定时竞争 | 低 | run 重复 | `AtomicBool` 互斥 + mpsc 统一调度 |

---

## 13. 实施顺序（writing-plans 阶段细化）

1. 新增 `training_export/` 模块骨架 + 公共类型
2. `sft_transform.rs` 纯函数 + 黄金对照测试（TDD）
3. `checkpoint.rs` 文件级增量
4. `runner.rs` 单次 run 编排（扫文件 + DB 查询 + transform + 写产物）
5. `scheduler.rs` 后台 task（interval + shutdown + mpsc）
6. `config.rs` 加载（env > yaml > default）
7. HTTP handlers + 路由注册
8. `main.rs` spawn 集成 + shutdown 集成
9. 集成测试（零热路径影响 benchmark、优雅关闭、错误隔离）
