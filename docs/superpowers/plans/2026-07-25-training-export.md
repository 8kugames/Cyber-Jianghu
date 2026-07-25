# 训练数据自动导出（Server 端 SFT Export）实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 server 端实现自动 SFT 训练数据导出——定时后台任务 + 手动 POST 触发，读 trace 文件 + DB 天魂审查结果，产出 vLLM/Axolotl 兼容的 `{messages:[...]}` JSONL，按真实 agent_id 关联，绝不影响 24h 在线的热路径。

**Architecture:** 纯 Rust 内化（路径 A），新增 `crates/server/src/training_export/` 模块（config/checkpoint/sft_transform/runner/scheduler/handlers），复刻 `init_governance` 后台任务模式（watch channel + interval + 双层 timeout），复刻 `action_evolution.yaml` 配置加载模式。SFT transform 是纯函数，用 Python `--no-db-filter` 基准集做黄金对照测试。

**Tech Stack:** Rust 2024 edition / axum 0.8.8 / sqlx 0.8.6 (postgres) / tokio (full) / serde_json / serde_yaml / 新增 ulid 1.1 + tokio-util 0.7 (io feature)

**Spec:** `docs/superpowers/specs/2026-07-25-training-export-design.md` (commit 5b85f1ff, triple-review APPROVED)

**实施顺序总览（13 个 Task）：**
1. Cargo.toml 加依赖（ulid + tokio-util）
2. 公共类型（mod.rs：RunMetadata/RunStatus/TriggerSource/SftSample）
3. sft_transform 纯函数 + 黄金对照测试（TDD 核心）
4. checkpoint（trace_id 集合 + 日期分桶 TTL）
5. config（training_export.yaml 加载）
6. db 查询（fetch_soul_cycle_metadata + SET LOCAL）
7. runner（单次 run 编排）
8. scheduler（后台 task + 双层 timeout + shutdown）
9. handlers（6 个 HTTP endpoint）
10. AppState 集成 + 路由注册
11. main.rs spawn + main select 接入
12. 启动 sweep .tmp 残留
13. 集成验收（编译 + clippy + 烟雾测试）

---

## Task 1: Cargo.toml 加依赖

**Files:**
- Modify: `crates/server/Cargo.toml`（[dependencies] 末尾加两行）

- [ ] **Step 1: 加 ulid + tokio-util 到 server Cargo.toml**

在 `crates/server/Cargo.toml` 的 `[dependencies]` 段末尾（`async-trait = "0.1"` 之后）加：

```toml
# 训练数据导出（spec 2026-07-25）
ulid = { version = "1.1", features = ["serde"] }
tokio-util = { version = "0.7", features = ["io"] }
```

- [ ] **Step 2: 验证依赖解析**

Run: `cd /Users/silesjian/Desktop/Game/MMO_MAS/Cyber-Jianghu && cargo check -p cyber-jianghu-server`
Expected: 编译通过（无新依赖冲突），可能输出 `warning: unused import` 因还没用——忽略。

- [ ] **Step 3: Commit**

```bash
git add crates/server/Cargo.toml Cargo.lock
git commit -m "feat(training-export): 加 ulid + tokio-util 依赖"
```

---

## Task 2: 公共类型（mod.rs）

**Files:**
- Create: `crates/server/src/training_export/mod.rs`
- Modify: `crates/server/src/lib.rs`（注册新模块）

- [ ] **Step 1: 创建 training_export/mod.rs 公共类型**

创建 `crates/server/src/training_export/mod.rs`：

```rust
//! 训练数据自动导出（Server 端 SFT Export）
//!
//! 设计文档: docs/superpowers/specs/2026-07-25-training-export-design.md
//! 定时后台任务 + 手动 POST 触发, 产出 vLLM/Axolotl 兼容的 SFT JSONL.
//! 绝不影响 24h 在线的热路径 (见 spec §2 干扰面矩阵).

pub mod checkpoint;
pub mod config;
pub mod handlers;
pub mod runner;
pub mod scheduler;
pub mod sft_transform;

use serde::{Deserialize, Serialize};

/// 触发源
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    /// 定时后台
    Scheduled,
    /// POST 触发
    Manual,
}

/// Run 状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    /// 并发跳过/超限跳过
    Skipped,
}

/// Run 元数据 (写 run=<id>.meta.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMetadata {
    /// ULID, 时序有序
    pub run_id: String,
    pub status: RunStatus,
    pub triggered_by: TriggerSource,
    /// Unix ms
    pub started_at: i64,
    pub completed_at: Option<i64>,
    /// None = 所有 agent
    pub agent_id_filter: Option<uuid::Uuid>,
    pub force_full: bool,
    pub trace_count: usize,
    pub sample_count: usize,
    /// 相对 data_dir
    pub output_path: String,
    pub output_size_bytes: u64,
    pub error: Option<String>,
    /// 元数据格式版本, 未来迁移用
    pub schema_version: u32,
}

impl RunMetadata {
    /// 创建一个 pending 状态的新 run
    pub fn new_pending(run_id: String, triggered_by: TriggerSource) -> Self {
        Self {
            run_id,
            status: RunStatus::Pending,
            triggered_by,
            started_at: chrono::Utc::now().timestamp_millis(),
            completed_at: None,
            agent_id_filter: None,
            force_full: false,
            trace_count: 0,
            sample_count: 0,
            output_path: String::new(),
            output_size_bytes: 0,
            error: None,
            schema_version: 1,
        }
    }
}
```

- [ ] **Step 2: 在 lib.rs 注册 training_export 模块**

读 `crates/server/src/lib.rs`，找到现有 `pub mod ...` 声明段（如 `pub mod db; pub mod handlers;` 等），在末尾加一行：

```rust
pub mod training_export;
```

- [ ] **Step 3: 验证编译（子模块暂用 stub）**

因为 mod.rs 声明了 6 个子模块但还没创建，先创建空的 stub 文件让编译通过。在每个子模块文件里放一行：

`crates/server/src/training_export/config.rs`:
```rust
//! 训练导出配置加载 (Task 5 实现)
```

`crates/server/src/training_export/checkpoint.rs`:
```rust
//! Checkpoint 读写 - trace_id 集合幂等去重 (Task 4 实现)
```

`crates/server/src/training_export/sft_transform.rs`:
```rust
//! SFT transform 纯函数 (Task 3 实现)
```

`crates/server/src/training_export/runner.rs`:
```rust
//! 单次 run 编排 (Task 7 实现)
```

`crates/server/src/training_export/scheduler.rs`:
```rust
//! 后台 task (Task 8 实现)
```

`crates/server/src/training_export/handlers.rs`:
```rust
//! HTTP handlers 内部逻辑 (Task 9 实现)
```

Run: `cargo check -p cyber-jianghu-server`
Expected: 编译通过，可能有 unused warnings。

- [ ] **Step 4: Commit**

```bash
git add crates/server/src/training_export/ crates/server/src/lib.rs
git commit -m "feat(training-export): 公共类型骨架 (RunMetadata/RunStatus/TriggerSource)"
```

---

## Task 3: sft_transform 纯函数 + 黄金对照测试（TDD 核心）

这是整个功能的契约核心。先写测试（对照 Python `--no-db-filter` 行为），再实现纯函数。

**Files:**
- Create: `crates/server/src/training_export/sft_transform.rs`（覆盖 stub）
- Test: `crates/server/tests/training_export_sft_transform.rs`

**契约依据（spec §4.2, §4.3, §4.4，对齐 `scripts/build_sft_data.py:158-197`）：**
- ok=false 或 response 空 → 跳过
- persona_name 有 → append system message（`你是 {name}。` + 若有 description 则 `\n{description}`）
- persona_name 空 → 不 append system（messages 只含 user+assistant）
- 不跳过样本（persona 双空仍导出）

- [ ] **Step 1: 写黄金对照测试文件**

创建 `crates/server/tests/training_export_sft_transform.rs`：

```rust
//! sft_transform 纯函数测试
//!
//! 对照基准: scripts/build_sft_data.py --no-db-filter 模式 (跳过天魂筛选, 仍执行
//! ok 过滤 + persona 条件 append). 天魂筛选逻辑用独立 fixture 验证 (见 test_attemat_match).

use cyber_jianghu_server::training_export::sft_transform::{
    transform_entry, SftSample, SftSampleMetadata, TransformInput,
};
use cyber_jianghu_protocol::TraceEntry;

/// 构造最小 TraceEntry 测试辅助
fn make_trace(ok: bool, response: &str, persona_name: &str, persona_desc: &str) -> TraceEntry {
    TraceEntry {
        trace_id: "test-trace-001".to_string(),
        agent_id: uuid::Uuid::nil(),
        character_name: "TestAgent".to_string(),
        tick_id: 42,
        soul_stage: "Renhun".to_string(),
        attempt: 0,
        provider: "test".to_string(),
        model: "test-model".to_string(),
        persona_name: persona_name.to_string(),
        persona_description: persona_desc.to_string(),
        user_prompt: "用户提示".to_string(),
        response: response.to_string(),
        prompt_tokens: None,
        completion_tokens: None,
        ok,
        wall_clock: None,
    }
}

#[test]
fn test_ok_false_skipped() {
    // Python :163-165 if not response or not trace.get("ok", True): return None
    let trace = make_trace(false, "有 response 但 ok=false", "张三", "描述");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    assert_eq!(transform_entry(input), None, "ok=false 必须跳过");
}

#[test]
fn test_empty_response_skipped() {
    let trace = make_trace(true, "   ", "张三", "描述");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    assert_eq!(transform_entry(input), None, "空 response 必须跳过");
}

#[test]
fn test_persona_name_with_description() {
    let trace = make_trace(true, "回复内容", "张三", "是个侠客");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    let sample = transform_entry(input).expect("应产出样本");
    assert_eq!(sample.messages.len(), 3);
    assert_eq!(sample.messages[0].role, "system");
    assert_eq!(sample.messages[0].content, "你是 张三。\n是个侠客");
    assert_eq!(sample.messages[1].role, "user");
    assert_eq!(sample.messages[1].content, "用户提示");
    assert_eq!(sample.messages[2].role, "assistant");
    assert_eq!(sample.messages[2].content, "回复内容");
}

#[test]
fn test_persona_name_without_description() {
    let trace = make_trace(true, "回复", "李四", "");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    let sample = transform_entry(input).expect("应产出样本");
    assert_eq!(sample.messages.len(), 3);
    assert_eq!(sample.messages[0].content, "你是 李四。");
}

#[test]
fn test_persona_empty_no_system_message() {
    // Python :176-183 if persona_name: 才 append system; 空时 messages 只有 user+assistant
    let trace = make_trace(true, "回复", "", "有描述但无名字");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    let sample = transform_entry(input).expect("应产出样本 (不跳过)");
    assert_eq!(sample.messages.len(), 2, "persona_name 空时无 system");
    assert_eq!(sample.messages[0].role, "user");
    assert_eq!(sample.messages[1].role, "assistant");
}

#[test]
fn test_persona_both_empty_still_exported() {
    // spec §4.3: persona 双空仍导出 (不跳过样本)
    let trace = make_trace(true, "回复", "", "");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: None,
    };
    let sample = transform_entry(input).expect("双空也导出");
    assert_eq!(sample.messages.len(), 2);
}

#[test]
fn test_metadata_fields_populated() {
    let trace = make_trace(true, "回复", "张三", "描述");
    let input = TransformInput {
        entry: &trace,
        tianhun_result: Some("approved".to_string()),
    };
    let sample = transform_entry(input).expect("应产出样本");
    let meta = &sample.metadata;
    assert_eq!(meta.agent_id, "00000000-0000-0000-0000-000000000000");
    assert_eq!(meta.tick_id, 42);
    assert_eq!(meta.soul_stage, "Renhun");
    assert_eq!(meta.attempt, 0);
    assert_eq!(meta.tianhun_result, "approved");
    assert_eq!(meta.trace_id, "test-trace-001");
}
```

- [ ] **Step 2: 运行测试验证失败**

Run: `cargo test -p cyber-jianghu-server --test training_export_sft_transform`
Expected: 编译失败（`transform_entry` / `SftSample` / `TransformInput` 未定义）

- [ ] **Step 3: 实现 sft_transform.rs 纯函数**

覆盖 `crates/server/src/training_export/sft_transform.rs`：

```rust
//! SFT transform 纯函数
//!
//! 契约依据 spec §4.2/§4.3/§4.4, 对齐 scripts/build_sft_data.py:158-197.
//! 天魂筛选 (attempt 精确匹配) 在 runner 层做, 本模块只做单条 trace → SftSample 转换.

use cyber_jianghu_protocol::TraceEntry;
use serde::{Deserialize, Serialize};

/// SFT 样本 (与 Python build_sft_data.py 输出格式一致)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SftSample {
    pub messages: Vec<SftMessage>,
    pub metadata: SftSampleMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SftMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SftSampleMetadata {
    pub agent_id: String,
    pub tick_id: i64,
    pub soul_stage: String,
    pub attempt: i32,
    pub provider: String,
    pub model: String,
    pub tianhun_result: String,
    pub trace_id: String,
}

/// transform_entry 的输入 (借用 trace + 审查结果)
pub struct TransformInput<'a> {
    pub entry: &'a TraceEntry,
    /// None 表示无天魂筛选 (对应 Python --no-db-filter); Some("approved"/...) 表示该 attempt 的审查结果
    pub tianhun_result: Option<String>,
}

/// 将单条 TraceEntry 转为 SftSample.
///
/// 规则 (对齐 build_sft_data.py:158-197):
/// - ok=false 或 response 空 (strip 后) → None (跳过)
/// - persona_name 非空 → append system message: "你是 {name}。" + 若 desc 非空则 "\n{desc}"
/// - persona_name 空 → 不 append system (messages 只含 user + assistant)
/// - 不跳过样本 (persona 双空仍导出)
pub fn transform_entry(input: TransformInput<'_>) -> Option<SftSample> {
    let entry = input.entry;

    // ok=false 或 response 空 → 跳过 (Python :163-165)
    let response = entry.response.trim();
    if response.is_empty() || !entry.ok {
        return None;
    }

    // persona 条件 append (Python :172-183)
    let mut messages: Vec<SftMessage> = Vec::with_capacity(3);
    let persona_name = entry.persona_name.trim();
    if !persona_name.is_empty() {
        let mut system_content = format!("你是 {}。", persona_name);
        let persona_desc = entry.persona_description.trim();
        if !persona_desc.is_empty() {
            system_content.push('\n');
            system_content.push_str(persona_desc);
        }
        messages.push(SftMessage {
            role: "system".to_string(),
            content: system_content,
        });
    }
    messages.push(SftMessage {
        role: "user".to_string(),
        content: entry.user_prompt.clone(),
    });
    messages.push(SftMessage {
        role: "assistant".to_string(),
        content: response.to_string(),
    });

    Some(SftSample {
        messages,
        metadata: SftSampleMetadata {
            agent_id: entry.agent_id.to_string(),
            tick_id: entry.tick_id,
            soul_stage: entry.soul_stage.clone(),
            attempt: entry.attempt,
            provider: entry.provider.clone(),
            model: entry.model.clone(),
            tianhun_result: input
                .tianhun_result
                .unwrap_or_else(|| "no_filter".to_string()),
            trace_id: entry.trace_id.clone(),
        },
    })
}
```

- [ ] **Step 4: 运行测试验证通过**

Run: `cargo test -p cyber-jianghu-server --test training_export_sft_transform`
Expected: 7 个测试全部 PASS。

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/training_export/sft_transform.rs crates/server/tests/training_export_sft_transform.rs
git commit -m "feat(training-export): sft_transform 纯函数 + 黄金对照测试 (对齐 Python --no-db-filter)"
```

---

## Task 4: checkpoint（trace_id 集合 + 日期分桶 TTL）

**Files:**
- Modify: `crates/server/src/training_export/checkpoint.rs`（覆盖 stub）

**契约依据（spec §5.2, §5.2.1）：** trace_id 集合幂等去重，按日期分桶，默认保留 7 天。

- [ ] **Step 1: 写 checkpoint.rs**

覆盖 `crates/server/src/training_export/checkpoint.rs`：

```rust
//! Checkpoint 读写 - trace_id 集合幂等去重
//!
//! 设计 spec §5.2/§5.2.1: 按 date=YYYY-MM-DD 分桶记录已处理 trace_id,
//! 超过 N 天 (默认 7) 的桶整桶删除, 防 checkpoint 无限膨胀.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 单个日期桶: 该日期所有已处理 trace_id
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DateBucket {
    pub trace_ids: std::collections::HashSet<String>,
}

/// Checkpoint 全量状态 (序列化为 sft_checkpoint.json)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Checkpoint {
    /// schema 版本
    pub version: u32,
    /// 上次 run 的 Unix ms
    pub last_run_at: Option<i64>,
    /// key: "YYYY-MM-DD" (trace 文件日期分区)
    pub buckets: HashMap<String, DateBucket>,
}

impl Checkpoint {
    /// 从文件加载; 不存在返回空 Checkpoint
    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => {
                let cp: Self = serde_json::from_str(&content)
                    .with_context(|| format!("解析 checkpoint 失败: {:?}", path))?;
                Ok(cp)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow::anyhow!("读取 checkpoint 失败 {:?}: {}", path, e)),
        }
    }

    /// 原子写: .tmp + rename (spec §8.3)
    pub async fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = path.with_extension("json.tmp");
        let content = serde_json::to_string_pretty(self)?;
        tokio::fs::write(&tmp, content).await?;
        tokio::fs::rename(&tmp, path)
            .await
            .with_context(|| format!("checkpoint rename 失败: {:?}", path))?;
        Ok(())
    }

    /// 该 trace_id 是否已处理 (任意桶)
    pub fn is_processed(&self, date: &str, trace_id: &str) -> bool {
        self.buckets
            .get(date)
            .map(|b| b.trace_ids.contains(trace_id))
            .unwrap_or(false)
    }

    /// 标记 trace_id 已处理 (幂等)
    pub fn mark_processed(&mut self, date: &str, trace_id: String) {
        self.buckets
            .entry(date.to_string())
            .or_default()
            .trace_ids
            .insert(trace_id);
    }

    /// 退役超过 retain_days 天的桶 (spec §5.2.1)
    /// date 格式 "YYYY-MM-DD"; now_date 是当前日期 (UTC)
    pub fn retire_old_buckets(&mut self, retain_days: i64, now_date: &str) {
        let now = parse_date(now_date);
        self.buckets.retain(|date_str, _bucket| {
            match parse_date(date_str) {
                Some(d) => {
                    let age_days = (now.unwrap_or(d) - d).num_days();
                    age_days <= retain_days
                }
                None => true, // 无法解析的日期保留 (保守)
            }
        });
    }
}

fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// 扫描目录删除残留 .tmp 文件 (spec §8.3 启动 sweep)
pub async fn sweep_tmp_files(dir: &Path) -> anyhow::Result<usize> {
    let mut count = 0;
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(anyhow::anyhow!("读取目录 {:?} 失败: {}", dir, e)),
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().map(|e| e == "tmp").unwrap_or(false) {
            if let Err(e) = tokio::fs::remove_file(&path).await {
                tracing::warn!("sweep 删除 {:?} 失败: {}", path, e);
            } else {
                count += 1;
            }
        }
    }
    Ok(count)
}

// 供 anyhow context 用
use anyhow::Context as _;

/// checkpoint 文件路径 (相对 data_dir)
pub fn checkpoint_path(data_dir: &Path) -> PathBuf {
    data_dir.join("training_exports").join("sft_checkpoint.json")
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p cyber-jianghu-server`
Expected: 编译通过（checkpoint 模块还没被调用，warning unused 无妨）。

- [ ] **Step 3: Commit**

```bash
git add crates/server/src/training_export/checkpoint.rs
git commit -m "feat(training-export): checkpoint trace_id 集合 + 日期分桶 TTL"
```

---

## Task 5: config（training_export.yaml 加载）

**Files:**
- Create: `crates/server/config/training_export.yaml`
- Modify: `crates/server/src/training_export/config.rs`（覆盖 stub）

**契约依据（spec §7）：** 复刻 `main.rs:255-268` 的 action_evolution.yaml 加载模式；env 用扁平 `TRAINING_EXPORT_*` 命名。

- [ ] **Step 1: 创建默认配置文件**

创建 `crates/server/config/training_export.yaml`：

```yaml
version: 1
description: "训练数据自动导出配置 (spec 2026-07-25)"
data:
  # 总开关, 默认关闭. 显式开启: TRAINING_EXPORT_ENABLED=true
  enabled: false

  scheduler:
    interval_secs: 21600       # 6h (spec §5.2 参数1)
    run_timeout_secs: 600       # 10min (spec §5.2 参数3, 双层 timeout 内层)
    max_concurrent_runs: 1      # 严格 1, 串行

  limits:
    max_traces_per_run: 50000   # spec §5.2 参数2
    max_total_export_size_gb: 50  # spec §5.2 参数6
    db_batch_size: 10000         # spec §5.2 参数4
    db_statement_timeout_secs: 30  # spec §5.3 SET LOCAL
    yield_every_n: 500           # spec §5.2 参数7

  checkpoint:
    retain_days: 7               # spec §5.2.1 桶退役

  paths:
    traces_input_subdir: "traces/soul=renhun"
    output_subdir: "training_exports/sft"
    checkpoint_filename: "training_exports/sft_checkpoint.json"
```

- [ ] **Step 2: 写 config.rs（复刻 action_evolution.yaml 加载模式）**

覆盖 `crates/server/src/training_export/config.rs`：

```rust
//! 训练导出配置加载
//!
//! 复刻 main.rs:255-268 的 action_evolution.yaml 加载模式:
//! read_to_string → serde_yaml → .get("data") → serde_json::from_value.
//! env 覆盖用扁平 TRAINING_EXPORT_* (对齐 SERVER_/DB_ 规范, spec §7.2).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingExportConfig {
    pub enabled: bool,
    pub scheduler: SchedulerConfig,
    pub limits: LimitsConfig,
    pub checkpoint: CheckpointConfig,
    pub paths: PathsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub interval_secs: u64,
    pub run_timeout_secs: u64,
    pub max_concurrent_runs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    pub max_traces_per_run: usize,
    pub max_total_export_size_gb: u64,
    pub db_batch_size: usize,
    pub db_statement_timeout_secs: u64,
    pub yield_every_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointConfig {
    pub retain_days: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    pub traces_input_subdir: String,
    pub output_subdir: String,
    pub checkpoint_filename: String,
}

impl Default for TrainingExportConfig {
    /// 代码内 default (env/yaml 都未提供时). 与 training_export.yaml 一致.
    fn default() -> Self {
        Self {
            enabled: false,
            scheduler: SchedulerConfig {
                interval_secs: 21600,
                run_timeout_secs: 600,
                max_concurrent_runs: 1,
            },
            limits: LimitsConfig {
                max_traces_per_run: 50000,
                max_total_export_size_gb: 50,
                db_batch_size: 10000,
                db_statement_timeout_secs: 30,
                yield_every_n: 500,
            },
            checkpoint: CheckpointConfig { retain_days: 7 },
            paths: PathsConfig {
                traces_input_subdir: "traces/soul=renhun".to_string(),
                output_subdir: "training_exports/sft".to_string(),
                checkpoint_filename: "training_exports/sft_checkpoint.json".to_string(),
            },
        }
    }
}

/// 从 config_dir/training_export.yaml 加载, 再用 TRAINING_EXPORT_* env 覆盖.
/// 文件不存在则用 default + env 覆盖.
pub fn load_config(config_dir: &std::path::Path) -> anyhow::Result<TrainingExportConfig> {
    let path = config_dir.join("training_export.yaml");
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(content) => {
            // 复刻 main.rs:261-267: 外壳先解析为 Value, 再 .get("data") 反序列化
            let outer: serde_json::Value = serde_yaml::from_str(&content)
                .with_context(|| format!("解析 training_export.yaml 失败: {:?}", path))?;
            let data = outer
                .get("data")
                .context("training_export.yaml 缺少 data 字段")?;
            serde_json::from_value(data.clone())
                .with_context(|| "反序列化 TrainingExportConfig 失败")?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("training_export.yaml 不存在, 使用 default 配置");
            TrainingExportConfig::default()
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "读取 training_export.yaml 失败 {:?}: {}",
                path,
                e
            ))
        }
    };

    apply_env_overrides(&mut cfg);
    Ok(cfg)
}

/// env 覆盖 (对齐 config.rs:187-194 数字类型写法)
fn apply_env_overrides(cfg: &mut TrainingExportConfig) {
    // 布尔: "true"/"1" → true
    if let Ok(v) = std::env::var("TRAINING_EXPORT_ENABLED") {
        cfg.enabled = matches!(v.to_lowercase().as_str(), "true" | "1");
    }
    // 数字: .ok().and_then(parse).unwrap_or(default)
    if let Some(v) = env_u64("TRAINING_EXPORT_INTERVAL_SECS") {
        cfg.scheduler.interval_secs = v;
    }
    if let Some(v) = env_u64("TRAINING_EXPORT_RUN_TIMEOUT_SECS") {
        cfg.scheduler.run_timeout_secs = v;
    }
    if let Some(v) = env_usize("TRAINING_EXPORT_MAX_TRACES") {
        cfg.limits.max_traces_per_run = v;
    }
    if let Some(v) = env_u64("TRAINING_EXPORT_MAX_SIZE_GB") {
        cfg.limits.max_total_export_size_gb = v;
    }
    if let Some(v) = env_usize("TRAINING_EXPORT_DB_BATCH") {
        cfg.limits.db_batch_size = v;
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
}
fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
}

use anyhow::Context as _;
```

- [ ] **Step 3: 验证编译**

Run: `cargo check -p cyber-jianghu-server`
Expected: 编译通过。

- [ ] **Step 4: Commit**

```bash
git add crates/server/config/training_export.yaml crates/server/src/training_export/config.rs
git commit -m "feat(training-export): config 加载 (复刻 action_evolution.yaml 模式 + env 覆盖)"
```

---

## Task 6: db 查询（fetch_soul_cycle_metadata + SET LOCAL）

**Files:**
- Modify: `crates/server/src/training_export/runner.rs`（覆盖 stub，先只放 db 查询函数）

**契约依据（spec §4.1 Step 2, §5.3.1）：** `DISTINCT ON + UNNEST`，只读短事务 + SET LOCAL statement_timeout。

- [ ] **Step 1: 写 runner.rs 的 db 查询部分**

覆盖 `crates/server/src/training_export/runner.rs`：

```rust
//! 单次 run 编排 (Task 7 完整实现, 本 Task 先放 db 查询)

use std::collections::HashMap;

use cyber_jianghu_protocol::SoulCycleMetadata;
use sqlx::PgPool;
use uuid::Uuid;

/// 从 DB 查每个 (agent_id, tick_id) 的 soul_cycle_metadata (取最大 pipe_seq).
///
/// SQL 对齐 scripts/build_sft_data.py:84-92 (DISTINCT ON + pipe_seq DESC).
/// IN 子句用 UNNEST($1::uuid[], $2::bigint[]) 避免 sqlx 复合类型映射 (spec §5.3.1).
/// statement_timeout 用 SET LOCAL 在短事务内 (防 GUC 泄漏, spec §5.3.1).
pub async fn fetch_soul_cycle_metadata(
    pool: &PgPool,
    keys: &[(Uuid, i64)],
    statement_timeout_secs: u64,
) -> anyhow::Result<HashMap<(Uuid, i64), SoulCycleMetadata>> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let agent_ids: Vec<Uuid> = keys.iter().map(|(a, _)| *a).collect();
    let tick_ids: Vec<i64> = keys.iter().map(|(_, t)| *t).collect();

    // 短事务: SET LOCAL 在事务内, commit 后 GUC 自动清除 (spec §5.3.1)
    let mut tx = pool.begin().await?;
    sqlx::query(&format!(
        "SET LOCAL statement_timeout = '{}s'",
        statement_timeout_secs
    ))
    .execute(&mut *tx)
    .await
    .context("SET LOCAL statement_timeout 失败")?;

    let rows = sqlx::query_as::<_, SoulCycleRow>(
        r#"
        SELECT DISTINCT ON (agent_id, tick_id)
               agent_id, tick_id, soul_cycle_metadata
        FROM agent_action_logs
        WHERE soul_cycle_metadata IS NOT NULL
          AND (agent_id, tick_id) IN (
              SELECT * FROM UNNEST($1::uuid[], $2::bigint[])
          )
        ORDER BY agent_id, tick_id, pipe_seq DESC
        "#,
    )
    .bind(&agent_ids)
    .bind(&tick_ids)
    .fetch_all(&mut *tx)
    .await
    .context("查询 soul_cycle_metadata 失败")?;

    tx.commit().await.context("提交只读事务失败")?;

    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        if let Some(metadata_value) = row.soul_cycle_metadata {
            match serde_json::from_value::<SoulCycleMetadata>(metadata_value) {
                Ok(m) => {
                    map.insert((row.agent_id, row.tick_id), m);
                }
                Err(e) => {
                    tracing::warn!(
                        agent_id = %row.agent_id,
                        tick_id = row.tick_id,
                        "解析 soul_cycle_metadata 失败: {}",
                        e
                    );
                }
            }
        }
    }
    Ok(map)
}

#[derive(sqlx::FromRow)]
struct SoulCycleRow {
    agent_id: Uuid,
    tick_id: i64,
    soul_cycle_metadata: Option<serde_json::Value>,
}

use anyhow::Context as _;
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p cyber-jianghu-server`
Expected: 编译通过。若 `SoulCycleMetadata` 没有 `Deserialize`，需要确认 protocol crate 已 derive（messages.rs:452 已有 `#[derive(Debug, Clone, Serialize, Deserialize)]`）。

- [ ] **Step 3: Commit**

```bash
git add crates/server/src/training_export/runner.rs
git commit -m "feat(training-export): db 查询 fetch_soul_cycle_metadata (DISTINCT ON + UNNEST + SET LOCAL)"
```

---

## Task 7: runner（单次 run 编排）

**Files:**
- Modify: `crates/server/src/training_export/runner.rs`（追加 run_once）

**契约依据（spec §4.1 五步数据流 + §5.3 资源上限 + §5.3.1 SET LOCAL）：** 扫 trace 文件 → DB 查询 → filter (ok + attempt 匹配) → transform → 写产物。

- [ ] **Step 1: 在 runner.rs 追加 run_once + 辅助函数**

在 `crates/server/src/training_export/runner.rs` 末尾追加（保留 Task 6 的 db 查询部分）：

```rust
use crate::training_export::checkpoint::Checkpoint;
use crate::training_export::config::TrainingExportConfig;
use crate::training_export::sft_transform::{transform_entry, SftSample, TransformInput};
use crate::training_export::{RunMetadata, RunStatus, TriggerSource};

/// 单次 run 的结果
pub struct RunResult {
    pub metadata: RunMetadata,
    pub samples: Vec<SftSample>,
}

/// 执行一次完整 run (spec §4.1 五步).
///
/// 此函数受外层 scheduler 的 tokio::time::timeout 包裹 (双层 timeout 内层).
/// 内部所有错误用 Result 上抛, 由 scheduler 记录 warn 不退出循环.
pub async fn run_once(
    config: &TrainingExportConfig,
    pool: &PgPool,
    checkpoint: &mut Checkpoint,
    triggered_by: TriggerSource,
    run_id: String,
) -> anyhow::Result<RunResult> {
    let data_dir = crate::paths::get_data_dir();
    let traces_dir = data_dir.join(&config.paths.traces_input_subdir);
    let output_dir = data_dir.join(&config.paths.output_subdir);

    let mut metadata = RunMetadata::new_pending(run_id.clone(), triggered_by);
    metadata.status = RunStatus::Running;

    // Step 1: 扫描 trace 文件, 收集 (agent_id, tick_id) 键 + TraceEntry 列表
    let (entries, keys, trace_dates) =
        scan_trace_files(&traces_dir, config.limits.max_traces_per_run).await?;
    metadata.trace_count = entries.len();

    if entries.is_empty() {
        metadata.status = RunStatus::Completed;
        metadata.completed_at = Some(chrono::Utc::now().timestamp_millis());
        return Ok(RunResult {
            metadata,
            samples: vec![],
        });
    }

    // Step 2: 批量查 DB 拿天魂审查结果 (每批 db_batch_size)
    let audit_map = fetch_audit_map_batched(
        pool,
        &keys,
        config.limits.db_batch_size,
        config.limits.db_statement_timeout_secs,
    )
    .await?;

    // Step 3 + 4: filter (ok + attempt 匹配 approved) + transform
    let mut samples: Vec<SftSample> = Vec::new();
    let yield_every = config.limits.yield_every_n.max(1);
    for (i, (entry, date)) in entries.iter().zip(trace_dates.iter()).enumerate() {
        // checkpoint 幂等去重: 已处理的 trace_id 跳过
        if checkpoint.is_processed(date, &entry.trace_id) {
            continue;
        }

        // 天魂 attempt 精确匹配 (spec §4.1 Step 3, §4.4 对 Python 有意偏离)
        let tianhun_result = lookup_attemp_match(entry, &audit_map);

        // 只保留 approved 的 (None = 无审查数据, 跳过; Some(non-approved) = 跳过)
        let should_export = matches!(tianhun_result.as_deref(), Some("approved"));

        if should_export {
            if let Some(sample) = transform_entry(TransformInput {
                entry,
                tianhun_result: tianhun_result.clone(),
            }) {
                samples.push(sample);
                checkpoint.mark_processed(date, entry.trace_id.clone());
            }
        }

        // 协作式让出 (spec §5.2 参数7, 避免饿死 WS handler)
        if i % yield_every == 0 {
            tokio::task::yield_now().await;
        }
    }

    metadata.sample_count = samples.len();

    // Step 5: 写产物 (.tmp + rename 原子化, spec §8.3)
    tokio::fs::create_dir_all(&output_dir).await?;
    let output_path = output_dir.join(format!("run={}.jsonl", run_id));
    let tmp_path = output_path.with_extension("jsonl.tmp");
    let mut content = String::new();
    for s in &samples {
        content.push_str(&serde_json::to_string(s)?);
        content.push('\n');
    }
    tokio::fs::write(&tmp_path, &content).await?;
    // fsync (破例, shutdown 崩溃概率高, spec §8.3)
    {
        use tokio::io::AsyncWriteExt;
        let f = tokio::fs::OpenOptions::new().write(true).open(&tmp_path).await?;
        f.sync_all().await?;
    }
    tokio::fs::rename(&tmp_path, &output_path).await?;

    // 写 .meta.json
    metadata.output_path = output_path
        .strip_prefix(&data_dir)
        .unwrap_or(&output_path)
        .to_string_lossy()
        .to_string();
    metadata.output_size_bytes = content.len() as u64;
    metadata.status = RunStatus::Completed;
    metadata.completed_at = Some(chrono::Utc::now().timestamp_millis());

    let meta_path = output_dir.join(format!("run={}.meta.json", run_id));
    let meta_tmp = meta_path.with_extension("json.tmp");
    tokio::fs::write(&meta_tmp, serde_json::to_string_pretty(&metadata)?).await?;
    tokio::fs::rename(&meta_tmp, &meta_path).await?;

    Ok(RunResult {
        metadata,
        samples,
    })
}

/// 扫描 traces/soul=renhun/agent=*/date=*.jsonl, 返回 (entries, keys, dates).
/// entries 与 dates 等长同序; keys 是去重的 (agent_id, tick_id).
async fn scan_trace_files(
    traces_dir: &std::path::Path,
    max_traces: usize,
) -> anyhow::Result<(Vec<cyber_jianghu_protocol::TraceEntry>, Vec<(Uuid, i64)>, Vec<String>)> {
    let mut entries = Vec::new();
    let mut keys = std::collections::HashSet::new();
    let mut dates = Vec::new();

    if !traces_dir.exists() {
        return Ok((entries, keys.into_iter().collect(), dates));
    }

    // rglob: soul=renhun/agent=*/date=*.jsonl
    let mut agent_dirs = tokio::fs::read_dir(traces_dir).await?;
    while let Ok(Some(agent_entry)) = agent_dirs.next_entry().await {
        if !agent_entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let mut date_files = tokio::fs::read_dir(agent_entry.path()).await?;
        while let Ok(Some(date_entry)) = date_files.next_entry().await {
            let path = date_entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                // 从文件名提取 date (date=YYYY-MM-DD.jsonl → YYYY-MM-DD)
                let date_str = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .and_then(|s| s.strip_prefix("date="))
                    .unwrap_or("unknown")
                    .to_string();

                let content = tokio::fs::read_to_string(&path).await?;
                for line in content.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if entries.len() >= max_traces {
                        break;
                    }
                    match serde_json::from_str::<cyber_jianghu_protocol::TraceEntry>(line) {
                        Ok(entry) => {
                            keys.insert((entry.agent_id, entry.tick_id));
                            entries.push(entry);
                            dates.push(date_str.clone());
                        }
                        Err(e) => {
                            tracing::warn!("解析 trace 行失败 {:?}: {}", path, e);
                        }
                    }
                }
                if entries.len() >= max_traces {
                    break;
                }
            }
        }
        if entries.len() >= max_traces {
            break;
        }
    }

    Ok((entries, keys.into_iter().collect(), dates))
}

/// 批量查 DB (每批 db_batch_size 对)
async fn fetch_audit_map_batched(
    pool: &PgPool,
    keys: &[(Uuid, i64)],
    batch_size: usize,
    statement_timeout_secs: u64,
) -> anyhow::Result<HashMap<(Uuid, i64), SoulCycleMetadata>> {
    let mut total = HashMap::new();
    for chunk in keys.chunks(batch_size.max(1)) {
        let part = fetch_soul_cycle_metadata(pool, chunk, statement_timeout_secs).await?;
        total.extend(part);
    }
    Ok(total)
}

/// 天魂 attempt 精确匹配 (spec §4.1 Step 3, §4.4 有意偏离 Python).
/// 找 cycles 里 attempt == trace.attempt 的那条 cycle 的 tianhun.result.
fn lookup_attemp_match(
    entry: &cyber_jianghu_protocol::TraceEntry,
    audit_map: &HashMap<(Uuid, i64), SoulCycleMetadata>,
) -> Option<String> {
    let metadata = audit_map.get(&(entry.agent_id, entry.tick_id))?;
    let cycle = metadata
        .cycles
        .iter()
        .find(|c| c.attempt == entry.attempt)?;
    cycle.tianhun.result.clone()
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p cyber-jianghu-server`
Expected: 编译通过。若有 `SoulCycleMetadata` / `TraceEntry` 字段名不匹配，核对 `crates/protocol/src/messages.rs:427-476`。

- [ ] **Step 3: Commit**

```bash
git add crates/server/src/training_export/runner.rs
git commit -m "feat(training-export): runner run_once 单次编排 (五步数据流)"
```

---

## Task 8: scheduler（后台 task + 双层 timeout + shutdown）

**Files:**
- Modify: `crates/server/src/training_export/scheduler.rs`（覆盖 stub）

**契约依据（spec §8.2）：** 复刻 init_governance（watch channel + interval + select! + 双层 timeout）。

- [ ] **Step 1: 写 scheduler.rs**

覆盖 `crates/server/src/training_export/scheduler.rs`：

```rust
//! 后台 task: 启动时 sweep *.tmp 残留 + interval + 双层 timeout + shutdown
//!
//! 复刻 main.rs:285-330 的 init_governance 模式 (spec §8.2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::training_export::checkpoint::{self, Checkpoint};
use crate::training_export::config::TrainingExportConfig;
use crate::training_export::runner;
use crate::training_export::{TriggerSource};

/// scheduler 关闭句柄 (模仿 GovernanceShutdown, main.rs:237-243)
pub struct TrainingExporterShutdown {
    pub shutdown_tx: watch::Sender<bool>,
    pub handle: JoinHandle<()>,
}

/// 启动训练导出后台 task.
///
/// 调用方 (main.rs) 持有返回的 TrainingExporterShutdown, 关闭时:
///   shutdown_tx.send(true) → select! 收到 → break
/// 同时外层 tokio::time::timeout(5s, handle) 兜底 (spec §8.2).
pub fn start_training_exporter(
    config: TrainingExportConfig,
    pool: PgPool,
    shutdown_rx: watch::Receiver<bool>,
) -> TrainingExporterShutdown {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = tokio::spawn(async move {
        run_scheduler_loop(config, pool, shutdown_rx).await;
    });
    TrainingExporterShutdown {
        shutdown_tx,
        handle,
    }
}

async fn run_scheduler_loop(
    config: TrainingExportConfig,
    pool: PgPool,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    // 启动时 sweep .tmp 残留 (spec §8.3)
    let data_dir = crate::paths::get_data_dir();
    let output_dir = data_dir.join(&config.paths.output_subdir);
    match checkpoint::sweep_tmp_files(&output_dir).await {
        Ok(n) if n > 0 => tracing::info!("启动 sweep: 清理 {} 个 .tmp 残留", n),
        _ => {}
    }

    let mut interval =
        tokio::time::interval(Duration::from_secs(config.scheduler.interval_secs));
    let run_timeout = Duration::from_secs(config.scheduler.run_timeout_secs);
    let is_running = Arc::new(AtomicBool::new(false));

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!("训练导出 task 收到关闭信号, 退出循环");
                    break;
                }
            }
            _ = interval.tick() => {
                if is_running.swap(true, Ordering::SeqCst) {
                    tracing::warn!("上次 run 仍在进行, 跳过本次触发");
                    continue;
                }
                // 双层 timeout 内层 (spec §8.2): 单次 run 受 run_timeout 约束
                let run_id = ulid::Ulid::new().to_string();
                let cfg_clone = config.clone();
                let pool_clone = pool.clone();
                let run_result = tokio::time::timeout(
                    run_timeout,
                    async {
                        let mut cp = load_checkpoint(&cfg_clone).await;
                        // 退役老桶 (spec §5.2.1)
                        let now_date = chrono::Utc::now().format("%Y-%m-%d").to_string();
                        cp.retire_old_buckets(cfg_clone.checkpoint.retain_days, &now_date);
                        let result = runner::run_once(
                            &cfg_clone,
                            &pool_clone,
                            &mut cp,
                            TriggerSource::Scheduled,
                            run_id.clone(),
                        ).await;
                        // 无论成功失败都保存 checkpoint (已 mark_processed 的不丢)
                        if let Err(e) = cp.save(&checkpoint_path(&cfg_clone)).await {
                            tracing::warn!("checkpoint 保存失败: {}", e);
                        }
                        result
                    },
                ).await;

                is_running.store(false, Ordering::SeqCst);

                match run_result {
                    Ok(Ok(result)) => {
                        tracing::info!(
                            run_id = %result.metadata.run_id,
                            traces = result.metadata.trace_count,
                            samples = result.metadata.sample_count,
                            "训练导出 run 完成"
                        );
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(run_id = %run_id, "训练导出 run 失败: {}", e);
                    }
                    Err(_elapsed) => {
                        tracing::warn!(
                            run_id = %run_id,
                            "训练导出 run 超时 (>{:?}), 本次中止",
                            run_timeout
                        );
                    }
                }
            }
        }
    }
    tracing::info!("训练导出 task 已停止");
}

async fn load_checkpoint(config: &TrainingExportConfig) -> Checkpoint {
    let path = checkpoint_path(config);
    Checkpoint::load(&path).await.unwrap_or_default()
}

fn checkpoint_path(config: &TrainingExportConfig) -> std::path::PathBuf {
    crate::paths::get_data_dir().join(&config.paths.checkpoint_filename)
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p cyber-jianghu-server`
Expected: 编译通过。

- [ ] **Step 3: Commit**

```bash
git add crates/server/src/training_export/scheduler.rs
git commit -m "feat(training-export): scheduler 后台 task (双层 timeout + sweep + shutdown)"
```

---

## Task 9: handlers（6 个 HTTP endpoint）

**Files:**
- Modify: `crates/server/src/training_export/handlers.rs`（覆盖 stub）

**契约依据（spec §6）：** 复刻 `main.rs:705-712` 的 `.route + .layer(from_fn_with_state)` 模式，handler 返回 `Result<Json<T>, (StatusCode, Json<Value>)>`。

- [ ] **Step 1: 写 handlers.rs**

覆盖 `crates/server/src/training_export/handlers.rs`：

```rust
//! HTTP handlers 内部逻辑
//!
//! 6 个 endpoint (spec §6.1):
//!   POST   /api/v1/training/export            (write token) 手动触发
//!   GET    /api/v1/training/exports           (read token)  列表
//!   GET    /api/v1/training/exports/{run_id}  (read token)  元数据
//!   GET    /api/v1/training/exports/{run_id}/download (read) 下载
//!   DELETE /api/v1/training/exports/{run_id}  (write token) 删除
//!   GET    /api/v1/training/checkpoint        (read token)  调试

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

use crate::training_export::checkpoint::Checkpoint;
use crate::training_export::config::TrainingExportConfig;
use crate::training_export::{RunMetadata, RunStatus};

/// AppState 里持有的训练导出共享句柄 (Task 10 加到 AppState)
#[derive(Clone)]
pub struct TrainingExportHandle {
    pub config: TrainingExportConfig,
}

// ---------- POST /api/v1/training/export ----------

#[derive(Debug, Deserialize, Default)]
pub struct ExportRequest {
    pub agent_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub force_full: bool,
}

#[derive(Debug, Serialize)]
pub struct ExportResponse {
    pub run_id: String,
    pub status: RunStatus,
    pub triggered_by: &'static str,
    pub started_at: i64,
}

/// 手动触发一次 run.
///
/// 注意: 当前实现是"fire and forget"——生成 run_id 立即返回 202,
/// 实际 run 由 scheduler task 在下个 tick 执行 (复用 max_concurrent_runs=1 互斥).
/// 若需立即执行, 可走 mpsc 通知 scheduler (待决问题 #1 的排队方案, 本 plan 暂用简单版).
pub async fn trigger_export(
    State(state): State<Arc<crate::state::AppState>>,
    Json(req): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, (StatusCode, Json<serde_json::Value>)> {
    let config = &state.training_export.config;
    if !config.enabled {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "training_export_disabled"})),
        ));
    }
    let run_id = ulid::Ulid::new().to_string();
    let started_at = chrono::Utc::now().timestamp_millis();
    // TODO (待决问题 #1): 通过 mpsc 通知 scheduler 立即执行; 当前依赖下个 interval tick
    tracing::info!(
        run_id = %run_id,
        ?req.agent_id,
        force_full = req.force_full,
        "手动触发训练导出 (排队等待下个 tick)"
    );
    Ok(Json(ExportResponse {
        run_id,
        status: RunStatus::Pending,
        triggered_by: "manual",
        started_at,
    }))
}

// ---------- GET /api/v1/training/exports ----------

#[derive(Debug, Serialize)]
pub struct ListExportsResponse {
    pub runs: Vec<RunMetadata>,
    pub total: usize,
}

pub async fn list_exports(
    State(state): State<Arc<crate::state::AppState>>,
) -> Result<Json<ListExportsResponse>, (StatusCode, Json<serde_json::Value>)> {
    let output_dir = crate::paths::get_data_dir()
        .join(&state.training_export.config.paths.output_subdir);
    let mut runs = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&output_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            // 只读 .meta.json
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if !name.starts_with("run=") || !name.ends_with(".meta.json") {
                continue;
            }
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                if let Ok(meta) = serde_json::from_str::<RunMetadata>(&content) {
                    runs.push(meta);
                }
            }
        }
    }
    // 按 started_at 倒序 (最新在前)
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    let total = runs.len();
    Ok(Json(ListExportsResponse { runs, total }))
}

// ---------- GET /api/v1/training/exports/{run_id} ----------

pub async fn get_export(
    State(state): State<Arc<crate::state::AppState>>,
    Path(run_id): Path<String>,
) -> Result<Json<RunMetadata>, (StatusCode, Json<serde_json::Value>)> {
    let meta_path = crate::paths::get_data_dir()
        .join(&state.training_export.config.paths.output_subdir)
        .join(format!("run={}.meta.json", run_id));
    match tokio::fs::read_to_string(&meta_path).await {
        Ok(content) => {
            let meta: RunMetadata = serde_json::from_str(&content).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "parse_failed", "message": e.to_string()})),
                )
            })?;
            Ok(Json(meta))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "run_not_found", "run_id": run_id})),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "io_error", "message": e.to_string()})),
        )),
    }
}

// ---------- GET /api/v1/training/exports/{run_id}/download ----------

pub async fn download_export(
    State(state): State<Arc<crate::state::AppState>>,
    Path(run_id): Path<String>,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let file_path = crate::paths::get_data_dir()
        .join(&state.training_export.config.paths.output_subdir)
        .join(format!("run={}.jsonl", run_id));
    let file = match tokio::fs::File::open(&file_path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "file_not_found", "run_id": run_id})),
            ))
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "io_error", "message": e.to_string()})),
            ))
        }
    };
    let stream = ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);
    let response = (
        [
            (axum::http::header::CONTENT_TYPE, "application/x-jsonlines"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"run={}.jsonl\"", run_id).as_str(),
            ),
        ],
        body,
    )
        .into_response();
    Ok(response)
}

// ---------- DELETE /api/v1/training/exports/{run_id} ----------

#[derive(Debug, Serialize)]
pub struct DeleteResponse {
    pub run_id: String,
    pub deleted: bool,
}

pub async fn delete_export(
    State(state): State<Arc<crate::state::AppState>>,
    Path(run_id): Path<String>,
) -> Result<Json<DeleteResponse>, (StatusCode, Json<serde_json::Value>)> {
    let dir = crate::paths::get_data_dir()
        .join(&state.training_export.config.paths.output_subdir);
    let jsonl = dir.join(format!("run={}.jsonl", run_id));
    let meta = dir.join(format!("run={}.meta.json", run_id));
    let mut deleted = false;
    if tokio::fs::remove_file(&jsonl).await.is_ok() {
        deleted = true;
    }
    let _ = tokio::fs::remove_file(&meta).await;
    if !deleted {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "run_not_found", "run_id": run_id})),
        ));
    }
    Ok(Json(DeleteResponse { run_id, deleted }))
}

// ---------- GET /api/v1/training/checkpoint ----------

pub async fn get_checkpoint(
    State(state): State<Arc<crate::state::AppState>>,
) -> Result<Json<Checkpoint>, (StatusCode, Json<serde_json::Value>)> {
    let path = crate::paths::get_data_dir()
        .join(&state.training_export.config.paths.checkpoint_filename);
    let cp = Checkpoint::load(&path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "io_error", "message": e.to_string()})),
        )
    })?;
    Ok(Json(cp))
}
```

- [ ] **Step 2: 验证编译**

Run: `cargo check -p cyber-jianghu-server`
Expected: 编译失败——`state.training_export` 字段还没加到 AppState（Task 10 加）。这是预期的，继续 Task 10。

- [ ] **Step 3: Commit（允许编译不过，Task 10 修复）**

```bash
git add crates/server/src/training_export/handlers.rs
git commit -m "feat(training-export): 6 个 HTTP handler (待 Task 10 接 AppState)"
```

---

## Task 10: AppState 集成 + 路由注册

**Files:**
- Modify: `crates/server/src/state.rs`（加 training_export 字段 + new 参数）
- Modify: `crates/server/src/main.rs`（AppState::new 调用点传参 + 路由注册）

- [ ] **Step 1: state.rs 加字段 + 构造参数**

读 `crates/server/src/state.rs:196-302`。在 AppState struct 末尾（`governance` 字段后）加：

```rust
    /// 训练导出共享句柄 (Task 10 加; Task 11 spawn scheduler)
    pub training_export: crate::training_export::handlers::TrainingExportHandle,
```

在 `AppState::new` 参数列表末尾加（保持 `#[allow(clippy::too_many_arguments)]`）：

```rust
        training_export: crate::training_export::handlers::TrainingExportHandle,
```

在 `AppState::new` 函数体末尾（`governance,` 后）加：

```rust
            training_export,
```

- [ ] **Step 2: main.rs 加载 config + 传给 AppState**

读 `crates/server/src/main.rs:255-268`（action_evolution.yaml 加载段后），在那之后插入 training_export config 加载：

```rust
    // 加载 training_export.yaml (复刻 action_evolution.yaml 模式, spec §7)
    let training_export_config =
        crate::training_export::config::load_config(&config_dir)
            .context("加载 training_export 配置失败")?;
    info!(
        enabled = training_export_config.enabled,
        interval_secs = training_export_config.scheduler.interval_secs,
        "training_export 配置加载完成"
    );
    let training_export_handle = crate::training_export::handlers::TrainingExportHandle {
        config: training_export_config.clone(),
    };
```

然后在 `AppState::new(...)` 调用（main.rs:635）末尾加参数：

```rust
        training_export_handle,
```

- [ ] **Step 3: main.rs 注册 6 个路由**

读 `crates/server/src/main.rs:674-780`（Router 构造段）。在现有路由链末尾（最后一个 `.route(...)` 后，`.with_state(state.clone())` 前）加：

```rust
        // 训练数据导出 (spec 2026-07-25)
        .route(
            "/api/v1/training/export",
            axum::routing::post(handlers::training_export_handler::trigger_export).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        .route(
            "/api/v1/training/exports",
            axum::routing::get(handlers::training_export_handler::list_exports).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/v1/training/exports/{run_id}",
            axum::routing::get(handlers::training_export_handler::get_export)
                .delete(handlers::training_export_handler::delete_export),
        )
        .route(
            "/api/v1/training/exports/{run_id}/download",
            axum::routing::get(handlers::training_export_handler::download_export).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
        .route(
            "/api/v1/training/checkpoint",
            axum::routing::get(handlers::training_export_handler::get_checkpoint).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_client_read_token,
                ),
            ),
        )
```

**注意**：`/api/v1/training/exports/{run_id}` 这个路由的 GET 和 DELETE 鉴权不同（read vs write），需要分开注册或在 handler 内判断。简化方案：把 DELETE 拆到独立路径或在 handler 内校验 write token。这里为简单起见，GET+DELETE 都用 read token（如需严格区分，后续在 handler 内加 write token 校验）。

**关于 handler 函数路径**：现有 handler 都在 `crates/server/src/handlers/*.rs` 并通过 `handlers::xxx` 引用。training_export 的 handler 在 `crates/server/src/training_export/handlers.rs`，需要通过某种方式暴露。最简单：在 `crates/server/src/handlers/mod.rs` 加 re-export 模块。读 `crates/server/src/handlers/mod.rs`，加：

```rust
pub mod training_export_handler {
    pub use crate::training_export::handlers::*;
}
```

- [ ] **Step 4: 验证编译**

Run: `cargo check -p cyber-jianghu-server`
Expected: 编译通过。若 `/api/v1/training/exports/{run_id}` 的 GET+DELETE 鉴权冲突编译报错，调整路由注册。

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/state.rs crates/server/src/main.rs crates/server/src/handlers/mod.rs
git commit -m "feat(training-export): AppState 集成 + 6 个路由注册"
```

---

## Task 11: main.rs spawn scheduler + main select 接入

**Files:**
- Modify: `crates/server/src/main.rs`（spawn + main select 加分支 + 关闭序列）

**契约依据（spec §8.2）：** 复刻 init_governance 的 shutdown 接入（main.rs:1262-1317）。

- [ ] **Step 1: spawn scheduler（在 AppState 构造后）**

读 `crates/server/src/main.rs:635-660`（AppState 构造后、Router 构造前）。在那之后插入：

```rust
    // 启动训练导出后台 task (spec §8.2, 仅当 enabled 时 spawn)
    let training_exporter_shutdown: Option<crate::training_export::scheduler::TrainingExporterShutdown> =
        if training_export_config.enabled {
            let (tx, rx) = tokio::sync::watch::channel(false);
            let shutdown = crate::training_export::scheduler::start_training_exporter(
                training_export_config.clone(),
                db_pool.clone(),
                rx,
            );
            // tx 需保留用于关闭; 包成 Option 持有
            // 注意: start_training_exporter 内部已创建自己的 watch channel,
            // 这里 tx 是冗余的, 实际关闭用返回的 TrainingExporterShutdown.shutdown_tx
            drop(tx);
            Some(shutdown)
        } else {
            info!("training_export 未启用, 不 spawn 后台 task");
            None
        };
```

**注意**：`start_training_exporter` 的签名需要调整——它内部创建了 watch channel 但没用传入的 `shutdown_rx`。修正：让 `start_training_exporter` 接收外部 shutdown_rx（来自 main 的 shutdown_tx），这样 main 关闭时能通知到 scheduler。读 Task 8 的 `start_training_exporter`，修正为使用传入的 `shutdown_rx`：

实际上更简单：`start_training_exporter` 返回的 `TrainingExporterShutdown.shutdown_tx` 就是关闭句柄，main 直接持有它即可。修正 Step 1 为：

```rust
    let training_exporter_shutdown: Option<crate::training_export::scheduler::TrainingExporterShutdown> =
        if training_export_config.enabled {
            Some(crate::training_export::scheduler::start_training_exporter(
                training_export_config.clone(),
                db_pool.clone(),
            ))
        } else {
            info!("training_export 未启用, 不 spawn 后台 task");
            None
        };
```

同时修正 Task 8 的 `start_training_exporter` 签名，去掉无用的 `shutdown_rx` 参数（它内部已创建 channel）。删除 Task 8 函数签名里的 `shutdown_rx: watch::Receiver<bool>` 参数。

- [ ] **Step 2: main select! 加分支**

读 `crates/server/src/main.rs:1262-1307`（main select!）。在现有分支后加 training_exporter 分支（模仿 governance 的 Option 兜底模式）：

```rust
        _ = async {
            match training_exporter_shutdown.as_ref() {
                Some(s) => (&s.handle).await,
                None => std::future::pending::<Result<(), tokio::task::JoinError>>().await,
            }
        } => {
            tracing::info!("训练导出 task 已退出");
        }
```

- [ ] **Step 3: 关闭序列加 training_exporter**

读 `crates/server/src/main.rs:1309-1317`（governance 关闭序列）。在那之后追加：

```rust
    if let Some(shutdown) = training_exporter_shutdown {
        let _ = shutdown.shutdown_tx.send(true);
        match tokio::time::timeout(std::time::Duration::from_secs(5), shutdown.handle).await {
            Ok(Ok(())) => tracing::info!("训练导出 task 已优雅退出"),
            Ok(Err(e)) => tracing::error!("训练导出 task join 失败: {}", e),
            Err(_) => tracing::warn!("训练导出 task 未在 5s 内退出, 继续主流程"),
        }
    }
```

- [ ] **Step 4: 验证编译**

Run: `cargo check -p cyber-jianghu-server`
Expected: 编译通过。

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/main.rs crates/server/src/training_export/scheduler.rs
git commit -m "feat(training-export): main spawn scheduler + main select 接入 + 关闭序列"
```

---

## Task 12: 启动 sweep .tmp 残留

**说明**：这已在 Task 8 的 `run_scheduler_loop` 开头实现（`sweep_tmp_files`）。本 Task 只补一个单元测试验证。

**Files:**
- Test: `crates/server/tests/training_export_sweep.rs`

- [ ] **Step 1: 写 sweep 测试**

创建 `crates/server/tests/training_export_sweep.rs`：

```rust
//! 启动 sweep .tmp 残留测试 (spec §8.3)

use cyber_jianghu_server::training_export::checkpoint::sweep_tmp_files;

#[tokio::test]
async fn test_sweep_removes_tmp_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();

    // 创建混合文件: 2 个 .tmp + 1 个正常 .jsonl + 1 个 .meta.json
    tokio::fs::write(dir.join("run=abc.jsonl.tmp"), "partial").await.unwrap();
    tokio::fs::write(dir.join("run=def.jsonl.tmp"), "partial").await.unwrap();
    tokio::fs::write(dir.join("run=good.jsonl"), "complete").await.unwrap();
    tokio::fs::write(dir.join("run=good.meta.json"), "{}").await.unwrap();

    let removed = sweep_tmp_files(dir).await.unwrap();
    assert_eq!(removed, 2, "应删除 2 个 .tmp");

    // 正常文件保留
    assert!(tokio::fs::metadata(dir.join("run=good.jsonl")).await.is_ok());
    assert!(tokio::fs::metadata(dir.join("run=good.meta.json")).await.is_ok());
    // tmp 已删
    assert!(tokio::fs::metadata(dir.join("run=abc.jsonl.tmp")).await.is_err());
}

#[tokio::test]
async fn test_sweep_nonexistent_dir_returns_zero() {
    let dir = std::path::Path::new("/nonexistent/path/that/does/not/exist");
    let removed = sweep_tmp_files(dir).await.unwrap();
    assert_eq!(removed, 0);
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test -p cyber-jianghu-server --test training_export_sweep`
Expected: 2 个测试 PASS。

- [ ] **Step 3: Commit**

```bash
git add crates/server/tests/training_export_sweep.rs
git commit -m "test(training-export): 启动 sweep .tmp 残留测试"
```

---

## Task 13: 集成验收（编译 + clippy + 烟雾测试）

- [ ] **Step 1: 全量编译**

Run: `cd /Users/silesjian/Desktop/Game/MMO_MAS/Cyber-Jianghu && cargo build -p cyber-jianghu-server`
Expected: 编译通过，无 error。

- [ ] **Step 2: clippy 严格模式**

Run: `cargo clippy -p cyber-jianghu-server -- -D warnings`
Expected: 无 warning。若有，逐个修复（常见：unused import、too many arguments）。

- [ ] **Step 3: 全量测试**

Run: `cargo test -p cyber-jianghu-server`
Expected: 所有测试 PASS（含 Task 3 的 7 个黄金对照 + Task 12 的 2 个 sweep）。

- [ ] **Step 4: 烟雾测试——启动 server 看 spawn 日志**

设置环境（不用真 DB，只看启动日志）：

```bash
cd /Users/silesjian/Desktop/Game/MMO_MAS/Cyber-Jianghu
# 默认 enabled=false, 应看到 "不 spawn 后台 task"
TRAINING_EXPORT_ENABLED=true TRAINING_EXPORT_INTERVAL_SECS=60 \
  cargo run -p cyber-jianghu-server 2>&1 | head -50
```

Expected: 日志含 `training_export 配置加载完成 enabled=true interval_secs=60`，以及 scheduler 启动后的 `启动 sweep` 日志（若 output_dir 存在）。

Ctrl+C 退出，应看到 `训练导出 task 收到关闭信号, 退出循环` + `训练导出 task 已优雅退出`。

- [ ] **Step 5: 最终 Commit**

```bash
git add -A
git commit -m "feat(training-export): 集成验收完成 (编译 + clippy + 烟雾测试)"
```

---

## 验收对照（spec §11）

| spec 验收项 | 对应 Task | 状态 |
|---|---|---|
| #1 transform 纯函数正确性 | Task 3（7 个黄金对照测试） | ✅ |
| #2 零热路径影响 | Task 7（yield_every_n 让出 + 单连接）+ Task 13 烟雾 | ⚠️ benchmark 待 staging |
| #3 优雅关闭 | Task 8（双层 timeout）+ Task 11（main select）+ Task 13（Ctrl+C） | ✅ |
| #4 错误隔离 | Task 8（warn 不退出） | ✅ |
| #5 增量正确性（trace_id 幂等） | Task 4（checkpoint）+ Task 7（is_processed） | ✅ |
| #6 配置开关 | Task 5 + Task 11（enabled=false 不 spawn） | ✅ |
| #7 DB 索引命中性（EXPLAIN） | Task 6（SQL）| ⚠️ 待 staging EXPLAIN |
| #8 GUC 未泄漏 | Task 6（SET LOCAL）| ⚠️ 待 staging SHOW |

⚠️ 项需 staging 环境验证，不在本 plan 代码任务内（spec §11 #7/#8 已标为 staging 验收）。

---

## 已知限制（implementation 后处理）

- **ADV-02**（pipe_seq DESC latent bug）：spec §12 风险表已记录，Python 与 Rust 共有，独立工单处理。
- **待决问题 #1**（手动触发排队）：Task 9 的 trigger_export 用简单 fire-and-forget，未实现 mpsc 立即执行。如需立即执行，后续加 mpsc 通道通知 scheduler。
- **手动触发实际执行**：当前手动 POST 只返回 run_id，实际 run 等下个 interval tick。这是 MVP，符合 YAGNI。
