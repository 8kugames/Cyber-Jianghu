# C 阶段：数据可达性 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让前端可直接通过 API 获取完整数据——关系图谱存储+同步、客户端鉴权、统一世界快照、缺口端点。

**Architecture:** 关系同步用全量快照策略（每游戏日上报，天然幂等，复刻 DailySummary 范本）。鉴权新增 client_read_token 配置档（仿 admin）。世界快照用只读事务隔离读。C0-C4 硬依赖链线性执行。

**Tech Stack:** Rust 2024, sqlx 0.8 (PostgreSQL), serde, axum, tokio

**前置基线:** `pure` 分支，A 阶段 10 commit 已落地，编译+测试全绿

**Spec:** `docs/superpowers/specs/2026-07-15-data-reachability-stage-c-design.md`

---

## 文件结构总览

**新建文件：**
- `crates/server/migrations/022_agent_relationships.sql` — 关系图谱表
- `crates/server/migrations/023_chronicle_period_unique.sql` — chronicle 幂等约束
- `crates/server/src/db/relationship_ops.rs` — 关系 CRUD
- `crates/server/src/handlers/agent_relationships.rs` — 关系读端点
- `crates/server/src/handlers/dashboard/world_snapshot.rs` — 统一世界快照
- `crates/server/src/handlers/dashboard/locations.rs` — 地点/地图端点
- `crates/server/src/handlers/dashboard/deaths.rs` — 死亡时间线
- `crates/server/src/handlers/dashboard/dialogues.rs` — 对话聚合视图

**修改文件：**
- `crates/protocol/src/messages.rs` — ClientMessage::RelationshipSnapshot variant
- `crates/agent/src/infra/transport/websocket.rs` — send_relationship_snapshot 方法
- `crates/agent/src/core/lifecycle/tick.rs` — 游戏日结束时触发关系上报
- `crates/server/src/db/mod.rs` — 导出 relationship_ops + run_migrations
- `crates/server/src/db/common.rs` — run_migrations 函数（复刻 entrypoint 逻辑）
- `crates/server/src/websocket/handler.rs` — handle_relationship_snapshot + match 分支
- `crates/server/src/handlers/auth.rs` — require_client_read_token middleware
- `crates/server/src/handlers/mod.rs` — 导出新 handler 模块
- `crates/server/src/config.rs` — client_read_token 配置项
- `crates/server/src/state.rs` — AppState.client_read_token
- `crates/server/src/main.rs` — 路由注册 + token 加载
- `crates/server/src/handlers/context.rs` — 占位符修复

---

## 第一组：C0 进程内迁移器

### Task 1: Rust 内嵌迁移执行（复刻 entrypoint 逻辑）

**背景：** docker-entrypoint.sh 在容器启动时全量跑 migrations/*.sql（幂等 SQL，无追踪表）。非 docker 部署（cargo run / 裸二进制）无迁移执行。方案：在 init_db_pool 后加 Rust 函数复刻 entrypoint 逻辑，**不引入 sqlx::migrate! 追踪表**（避免与 entrypoint 冲突）。

**Files:**
- Modify: `crates/server/src/db/common.rs:84-143`（init_db_pool 后加 run_migrations）
- Modify: `crates/server/src/db/mod.rs`（导出）
- Modify: `crates/server/src/main.rs:462`（init_db_pool 后调 run_migrations）

- [ ] **Step 1: 在 common.rs 实现 run_migrations**

`crates/server/src/db/common.rs`，在 `init_db_pool` 函数后添加：
```rust
/// 执行 migrations 目录下的所有 SQL 文件（复刻 docker-entrypoint.sh 逻辑）。
///
/// 全量重跑幂等 SQL（与 entrypoint 行为一致），不引入 _sqlx_migrations 追踪表。
/// 非 docker 部署（cargo run / 裸二进制）靠此函数保证 schema 就绪。
pub async fn run_migrations(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    let migration_dir = std::path::Path::new("crates/server/migrations");
    if !migration_dir.is_dir() {
        tracing::warn!("迁移目录不存在: {:?}（docker 部署由 entrypoint 处理）", migration_dir);
        return Ok(());
    }

    let mut files: Vec<_> = std::fs::read_dir(migration_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort_by_key(|e| e.path());

    for file in &files {
        let filename = file.file_name().to_string_lossy().to_string();
        let sql = std::fs::read_to_string(file.path())?;
        tracing::info!("[migration] 执行: {}", filename);
        sqlx::query(&sql)
            .execute(pool)
            .await
            .map_err(|e| anyhow::anyhow!("迁移失败 {}: {}", filename, e))?;
    }
    tracing::info!("[migration] 全部完成 ({} 个文件)", files.len());
    Ok(())
}
```

- [ ] **Step 2: 导出 run_migrations**

`crates/server/src/db/mod.rs`：在 `pub use common::run_migrations;` 或确保 mod 声明覆盖。检查 `db/mod.rs` 的 `pub use` / `pub mod common`。

- [ ] **Step 3: 在 main.rs 启动流程调用**

`crates/server/src/main.rs:462`（init_db_pool 后），加：
```rust
let db_pool = init_db_pool(&config.database).await?;
// C0: 非 docker 部署时自动跑迁移（docker 由 entrypoint 处理，幂等 SQL 可安全重复）
crate::db::run_migrations(&db_pool).await?;
```

- [ ] **Step 4: 编译验证**

Run: `cargo check --workspace`
Expected: 编译通过

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(server): Rust 内嵌迁移执行——复刻 entrypoint 逻辑

非 docker 部署（cargo run/裸二进制）无迁移执行，CLAUDE.md 文档与实现有偏差。
在 init_db_pool 后加 run_migrations，全量重跑幂等 SQL（与 entrypoint 一致），
不引入 _sqlx_migrations 追踪表（避免与 entrypoint 冲突）。"
```

---

## 第二组：C1 关系图谱存储 + 同步 + 端点

### Task 2: 关系表 migration（022）

**Files:**
- Create: `crates/server/migrations/022_agent_relationships.sql`

- [ ] **Step 1: 写 migration SQL**

创建 `crates/server/migrations/022_agent_relationships.sql`：
```sql
-- 022: Agent 关系图谱（全量快照同步，source agent 对 target agent 的单向关系）
-- 幂等：与 entrypoint 全量重跑兼容

CREATE TABLE IF NOT EXISTS agent_relationships (
    source_agent_id      UUID NOT NULL,
    target_agent_id      UUID NOT NULL,
    target_name          TEXT NOT NULL DEFAULT '',
    favorability         INTEGER NOT NULL DEFAULT 0 CHECK(favorability >= -100 AND favorability <= 100),
    last_interaction_tick BIGINT NOT NULL DEFAULT 0,
    synced_at            BIGINT NOT NULL,
    self_description     TEXT NOT NULL DEFAULT '',
    description_tick     BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (source_agent_id, target_agent_id)
);

CREATE TABLE IF NOT EXISTS agent_relationship_key_events (
    id                   BIGSERIAL PRIMARY KEY,
    source_agent_id      UUID NOT NULL,
    target_agent_id      UUID NOT NULL,
    tick_id              BIGINT NOT NULL,
    event_type           TEXT NOT NULL,
    description          TEXT NOT NULL,
    favorability_delta   INTEGER NOT NULL,
    event_timestamp      BIGINT NOT NULL,
    FOREIGN KEY (source_agent_id, target_agent_id)
        REFERENCES agent_relationships(source_agent_id, target_agent_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_rel_events_source_target
    ON agent_relationship_key_events(source_agent_id, target_agent_id);
CREATE INDEX IF NOT EXISTS idx_rel_events_tick
    ON agent_relationship_key_events(tick_id DESC);
CREATE INDEX IF NOT EXISTS idx_relationships_source
    ON agent_relationships(source_agent_id);
```

- [ ] **Step 2: 验证 SQL 语法**

Run: `cargo check --workspace`（确认无 Rust 变化，SQL 文件不影响编译）
确认 migration 目录排序正确（022 在 021 之后）。

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(db): migration 022 agent_relationships 关系图谱表"
```

---

### Task 3: protocol ClientMessage::RelationshipSnapshot

**Files:**
- Modify: `crates/protocol/src/messages.rs:399`（DailySummary variant 后）

- [ ] **Step 1: 新增 RelationshipSnapshot variant**

`crates/protocol/src/messages.rs`，在 `DailySummary` variant（约 399 行）后添加：
```rust
    /// Agent 关系图谱快照（每游戏日全量上报，server 全量覆盖）
    RelationshipSnapshot {
        agent_id: uuid::Uuid,
        game_day: i64,
        relationships: Vec<crate::types::RelationshipMemory>,
    },
```

- [ ] **Step 2: 编译验证**

Run: `cargo check --workspace`

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(protocol): ClientMessage::RelationshipSnapshot variant"
```

---

### Task 4: server 端关系 CRUD（relationship_ops.rs）

**Files:**
- Create: `crates/server/src/db/relationship_ops.rs`
- Modify: `crates/server/src/db/mod.rs`

- [ ] **Step 1: 实现 upsert + list + get**

创建 `crates/server/src/db/relationship_ops.rs`：
```rust
use crate::models::agent::AgentUuid;
use anyhow::Result;
use cyber_jianghu_protocol::types::{RelationshipKeyEvent, RelationshipMemory};
use sqlx::PgPool;

/// 全量覆盖一个 source agent 的关系快照（DELETE + INSERT，镜像 agent 本地 upsert_relationship 语义）。
/// 幂等：同一快照重报不产生重复行。
pub async fn upsert_relationship_snapshot(
    pool: &PgPool,
    source_agent_id: &AgentUuid,
    game_day: i64,
    relationships: &[RelationshipMemory],
) -> Result<()> {
    let synced_at = chrono::Utc::now().timestamp_millis();

    let mut tx = pool.begin().await?;

    // 删除旧关系 + key_events（CASCADE）
    sqlx::query("DELETE FROM agent_relationships WHERE source_agent_id = $1")
        .bind(source_agent_id.0)
        .execute(&mut *tx)
        .await?;

    // 插入新快照
    for rel in relationships {
        sqlx::query(
            r#"INSERT INTO agent_relationships
               (source_agent_id, target_agent_id, target_name, favorability,
                last_interaction_tick, synced_at, self_description, description_tick)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        )
        .bind(source_agent_id.0)
        .bind(rel.target_agent_id)
        .bind(&rel.target_name)
        .bind(rel.favorability)
        .bind(rel.last_interaction_tick)
        .bind(synced_at)
        .bind(&rel.self_description)
        .bind(rel.description_tick)
        .execute(&mut *tx)
        .await?;

        for ev in &rel.key_events {
            sqlx::query(
                r#"INSERT INTO agent_relationship_key_events
                   (source_agent_id, target_agent_id, tick_id, event_type,
                    description, favorability_delta, event_timestamp)
                   VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
            )
            .bind(source_agent_id.0)
            .bind(rel.target_agent_id)
            .bind(ev.tick_id)
            .bind(&ev.event_type)
            .bind(&ev.description)
            .bind(ev.favorability_delta)
            .bind(ev.timestamp)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    tracing::info!(
        "[relationship] 同步完成: agent={} game_day={} 关系数={}",
        source_agent_id.0, game_day, relationships.len()
    );
    Ok(())
}

/// 查询一个 source agent 的所有关系（含 key_events）。
pub async fn get_relationships_by_agent(
    pool: &PgPool,
    source_agent_id: &AgentUuid,
) -> Result<Vec<RelationshipMemory>> {
    let rels: Vec<(uuid::Uuid, String, i32, i64, i64, String, i64)> = sqlx::query_as(
        r#"SELECT target_agent_id, target_name, favorability, last_interaction_tick,
                  synced_at, self_description, description_tick
           FROM agent_relationships
           WHERE source_agent_id = $1
           ORDER BY favorability DESC"#,
    )
    .bind(source_agent_id.0)
    .fetch_all(pool)
    .await?;

    let mut result = Vec::new();
    for (target_id, target_name, favorability, last_tick, synced, desc, desc_tick) in rels {
        let events: Vec<(i64, String, String, i32, i64)> = sqlx::query_as(
            r#"SELECT tick_id, event_type, description, favorability_delta, event_timestamp
               FROM agent_relationship_key_events
               WHERE source_agent_id = $1 AND target_agent_id = $2
               ORDER BY tick_id DESC LIMIT 20"#,
        )
        .bind(source_agent_id.0)
        .bind(target_id)
        .fetch_all(pool)
        .await?;

        result.push(RelationshipMemory {
            target_agent_id: target_id,
            target_name,
            favorability,
            key_events: events
                .into_iter()
                .map(|(t, et, d, fd, ts)| RelationshipKeyEvent {
                    tick_id: t,
                    event_type: et,
                    description: d,
                    favorability_delta: fd,
                    timestamp: ts,
                })
                .collect(),
            last_interaction_tick: last_tick,
            updated_at: synced,
            self_description: desc,
            description_tick: desc_tick,
        });
    }
    Ok(result)
}

/// 查询所有 agent 的关系（用于全局关系图谱视图）。
pub async fn get_all_relationships(pool: &PgPool) -> Result<Vec<(uuid::Uuid, RelationshipMemory)>> {
    let sources: Vec<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT source_agent_id FROM agent_relationships ORDER BY source_agent_id",
    )
    .fetch_all(pool)
    .await?;

    let mut result = Vec::new();
    for (source_id,) in sources {
        let agent_uuid = AgentUuid(source_id);
        let rels = get_relationships_by_agent(pool, &agent_uuid).await?;
        for rel in rels {
            result.push((source_id, rel));
        }
    }
    Ok(result)
}
```

- [ ] **Step 2: 导出模块**

`crates/server/src/db/mod.rs`：添加 `pub mod relationship_ops;` 和必要的 re-export。

- [ ] **Step 3: 编译验证**

Run: `cargo check --workspace`
（注意：`AgentUuid` 的实际定义需确认——可能在 `models/agent.rs`，若不存在则直接用 `uuid::Uuid`）

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(db): relationship_ops——关系图谱 CRUD（全量覆盖+查询）"
```

---

### Task 5: server 端 handle_relationship_snapshot

**Files:**
- Modify: `crates/server/src/websocket/handler.rs:912`（DailySummary match arm 后）
- Modify: `crates/server/src/websocket/handler.rs:1680`（handle_daily_summary 旁）

- [ ] **Step 1: 实现 handle_relationship_snapshot**

`crates/server/src/websocket/handler.rs`，在 `handle_daily_summary`（约 1680 行）后添加：
```rust
async fn handle_relationship_snapshot(
    device_id: &str,
    snapshot: cyber_jianghu_protocol::ClientMessage::RelationshipSnapshot_fields, // 见下方说明
    state: &Arc<AppState>,
) -> Result<(), String> {
    let agent_id = match crate::db::get_agent_by_device_id(&state.db_pool, device_id).await {
        Ok(Some(agent)) => agent.agent_id,
        Ok(None) => return Err("无关联角色".into()),
        Err(e) => return Err(format!("查询角色失败: {}", e)),
    };

    // 归属校验：消息里的 agent_id 必须匹配 device 绑定的 agent
    if snapshot.agent_id != agent_id {
        return Err("关系快照 agent_id 与 device 不匹配".into());
    }

    crate::db::relationship_ops::upsert_relationship_snapshot(
        &state.db_pool,
        &agent_id,
        snapshot.game_day,
        &snapshot.relationships,
    )
    .await
    .map_err(|e| format!("关系快照持久化失败: {}", e))?;

    Ok(())
}
```

注意：`ClientMessage::RelationshipSnapshot` 的字段无法直接 struct 提取，需在 match arm 内解构。参照 handle_daily_summary 的写法（它在 match arm 里直接调用，参数内联）。

- [ ] **Step 2: 在 handle_client_message 加 match arm**

`crates/server/src/websocket/handler.rs:912`（DailySummary arm 后），添加：
```rust
        ClientMessage::RelationshipSnapshot { agent_id, game_day, relationships } => {
            if let Err(e) = handle_relationship_snapshot(
                device_id,
                agent_id,
                game_day,
                &relationships,
                &state,
            ).await {
                tracing::warn!("[relationship] 处理快照失败: {}", e);
            }
        }
```

调整 handle_relationship_snapshot 签名为接收解构后的参数（agent_id, game_day, relationships slice）。

- [ ] **Step 3: 编译验证**

Run: `cargo check --workspace`

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(server): handle_relationship_snapshot——接收全量快照并持久化"
```

---

### Task 6: agent 端 send_relationship_snapshot + 游戏日触发

**Files:**
- Modify: `crates/agent/src/infra/transport/websocket.rs:642`（send_daily_summary 旁）
- Modify: `crates/agent/src/infra/transport/websocket.rs:1219`（ManagedClient wrapper 旁）
- Modify: `crates/agent/src/core/lifecycle/tick.rs:88-127`（send_daily_summary 后）

- [ ] **Step 1: 在 websocket.rs Client impl 加 send_relationship_snapshot**

`crates/agent/src/infra/transport/websocket.rs`，在 `send_daily_summary`（约 642 行）后添加：
```rust
/// 上报关系图谱快照（每游戏日全量，server 全量覆盖）。
pub async fn send_relationship_snapshot(
    &self,
    agent_id: uuid::Uuid,
    game_day: i64,
    relationships: &[cyber_jianghu_protocol::types::RelationshipMemory],
) -> Result<()> {
    let msg = cyber_jianghu_protocol::ClientMessage::RelationshipSnapshot {
        agent_id,
        game_day,
        relationships: relationships.to_vec(),
    };
    self.send_client_message(msg).await
}
```

- [ ] **Step 2: 在 ManagedClient wrapper 加同名方法**

`crates/agent/src/infra/transport/websocket.rs`，在 ManagedClient 的 `send_daily_summary`（约 1219 行）后添加：
```rust
pub async fn send_relationship_snapshot(
    &self,
    agent_id: uuid::Uuid,
    game_day: i64,
    relationships: &[cyber_jianghu_protocol::types::RelationshipMemory],
) -> Result<()> {
    self.read().await.client
        .send_relationship_snapshot(agent_id, game_day, relationships)
        .await
}
```

- [ ] **Step 3: 在 tick.rs 游戏日结束时触发上报**

`crates/agent/src/core/lifecycle/tick.rs:88-127`（send_daily_summary 重试循环后），追加关系快照上报：
```rust
// 关系图谱全量快照上报（与 DailySummary 同批，每游戏日一次）
if let Some(store) = &self.relationship_store {
    match store.get_all_relationships() {
        Ok(rels) => {
            if !rels.is_empty() {
                let agent_id = self.agent_id; // 确认字段名
                if let Err(e) = self.client
                    .send_relationship_snapshot(agent_id, summary_game_day, &rels)
                    .await
                {
                    tracing::warn!("[relationship] 快照上报失败: {}", e);
                }
            }
        }
        Err(e) => tracing::warn!("[relationship] 读取关系失败: {}", e),
    }
}
```

注意：需确认 `self.relationship_store` 和 `self.agent_id` 的实际字段路径（可能在 lifecycle 结构体上，或通过其他方式访问）。读 tick.rs 上下文确认。

**类型转换**：agent 本地 `RelationshipMemory`（`DateTime<Utc>`）需转为 protocol `RelationshipMemory`（`i64` 毫秒）。get_all_relationships 返回 agent 本地类型，需映射：
```rust
let protocol_rels: Vec<_> = rels.iter().map(|r| {
    cyber_jianghu_protocol::types::RelationshipMemory {
        target_agent_id: r.target_agent_id,
        target_name: r.target_name.clone(),
        favorability: r.favorability,
        key_events: r.key_events.iter().map(|e| {
            cyber_jianghu_protocol::types::RelationshipKeyEvent {
                tick_id: e.tick_id,
                event_type: e.event_type.clone(),
                description: e.description.clone(),
                favorability_delta: e.favorability_delta,
                timestamp: e.timestamp.timestamp_millis(),
            }
        }).collect(),
        last_interaction_tick: r.last_interaction_tick,
        updated_at: r.updated_at.timestamp_millis(),
        self_description: r.self_description.clone(),
        description_tick: r.description_tick,
    }
}).collect();
```

- [ ] **Step 4: 编译验证**

Run: `cargo check --workspace`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(agent): 游戏日结束时上报关系图谱全量快照"
```

---

### Task 7: server 端关系读端点

**Files:**
- Create: `crates/server/src/handlers/agent_relationships.rs`
- Modify: `crates/server/src/handlers/mod.rs`
- Modify: `crates/server/src/main.rs`（路由注册）

- [ ] **Step 1: 实现读端点 handler**

创建 `crates/server/src/handlers/agent_relationships.rs`（范本：`handlers/agent_daily_summaries.rs`）：
```rust
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use std::sync::Arc;

use crate::state::AppState;

/// GET /api/dashboard/agent-relationships —— 全局关系图谱（所有 agent 的所有关系）
pub async fn get_all_relationships(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<(uuid::Uuid, cyber_jianghu_protocol::types::RelationshipMemory)>>, StatusCode> {
    let rels = crate::db::relationship_ops::get_all_relationships(&state.db_pool)
        .await
        .map_err(|e| {
            tracing::error!("查询全局关系失败: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(rels))
}

/// GET /api/dashboard/agent-relationships/{agent_id} —— 单个 agent 的所有关系
pub async fn get_relationships_by_agent(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<uuid::Uuid>,
) -> Result<Json<Vec<cyber_jianghu_protocol::types::RelationshipMemory>>, StatusCode> {
    let rels = crate::db::relationship_ops::get_relationships_by_agent(
        &state.db_pool,
        &crate::models::agent::AgentUuid(agent_id),
    )
    .await
    .map_err(|e| {
        tracing::error!("查询 agent 关系失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(rels))
}
```

- [ ] **Step 2: 导出模块**

`crates/server/src/handlers/mod.rs`：添加 `pub mod agent_relationships;` 和 `pub use agent_relationships::*;`（参照现有模块导出模式）。

- [ ] **Step 3: 注册路由**

`crates/server/src/main.rs`，在 dashboard 路由区（约 950 行 daily_summaries 路由旁）添加：
```rust
.route(
    "/api/dashboard/agent-relationships",
    get(handlers::get_all_relationships).layer(axum::middleware::from_fn_with_state(
        state.clone(),
        handlers::auth::require_read_token,
    )),
)
.route(
    "/api/dashboard/agent-relationships/{agent_id}",
    get(handlers::get_relationships_by_agent).layer(axum::middleware::from_fn_with_state(
        state.clone(),
        handlers::auth::require_read_token,
    )),
)
```

- [ ] **Step 4: 编译验证**

Run: `cargo check --workspace`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(server): 关系图谱读端点 GET /api/dashboard/agent-relationships"
```

---

## 第三组：C2 游戏客户端鉴权档

### Task 8: client_read_token 配置 + middleware

**Files:**
- Modify: `crates/server/src/config.rs:52`（ServerConfig 加字段）
- Modify: `crates/server/src/config.rs:162`（环境变量加载）
- Modify: `crates/server/src/state.rs:229`（AppState 加字段）
- Modify: `crates/server/src/main.rs:555`（token 加载）
- Modify: `crates/server/src/handlers/auth.rs:48`（middleware）
- Modify: `crates/server/src/main.rs`（dashboard READ 路由改鉴权 layer）

- [ ] **Step 1: config.rs 加 client_read_token 字段**

`crates/server/src/config.rs`，ServerConfig struct（约 52 行）加：
```rust
/// 游戏客户端只读 token（独立于 admin，前端专用）
pub client_read_token: Option<String>,
```

环境变量加载区（约 162 行）加：
```rust
client_read_token: std::env::var("CLIENT_READ_TOKEN").ok().filter(|s| !s.is_empty()),
```

- [ ] **Step 2: state.rs + main.rs 加 AppState 字段**

`crates/server/src/state.rs`（约 229 行）加：
```rust
pub client_read_token: Option<String>,
```

`crates/server/src/main.rs`（约 555 行）token 加载区，加：
```rust
let client_read_token = config.server.client_read_token.clone();
// 注意：client_read_token 未配置时为 None（不像 admin 那样自动随机生成——
// 因为它是可选的前端鉴权，不配置意味着禁用客户端鉴权档）
```

- [ ] **Step 3: auth.rs 加 require_client_read_token middleware**

`crates/server/src/handlers/auth.rs`，在 `require_read_token`（约 48 行）后添加：
```rust
/// 接受 client_read_token 或 admin_read_token（R/RW）。
/// 游戏客户端前端专用鉴权档。
pub async fn require_client_read_token(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let headers = req.headers();

    // 先尝试 client token
    if let Some(ref client_token) = state.client_read_token {
        if let Some(token) = extract_bearer_token(headers) {
            if token == client_token {
                return Ok(next.run(req).await);
            }
        }
    }

    // 回退到 admin read token
    if authenticate_admin_token(headers, &state.admin_read_token, &state.admin_write_token, false) {
        return Ok(next.run(req).await);
    }

    Err(StatusCode::UNAUTHORIZED)
}
```

注意：需确认 `extract_bearer_token` 是否可复用——读 `authenticate_admin_token`（auth.rs:27）的实现，提取 token 解析逻辑。

- [ ] **Step 4: dashboard READ 路由改鉴权 layer**

`crates/server/src/main.rs`，dashboard READ 路由的 `require_read_token` 改为 `require_client_read_token`（接受 client 或 admin）。保留 WRITE 路由用 `require_write_token` 不变。

- [ ] **Step 5: 编译验证**

Run: `cargo check --workspace`

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(server): client_read_token 鉴权档——前端专用只读访问"
```

---

## 第四组：C3 统一世界快照 + C4 缺口端点

### Task 9: 统一世界快照端点（只读事务隔离）

**Files:**
- Create: `crates/server/src/handlers/dashboard/world_snapshot.rs`
- Modify: `crates/server/src/handlers/mod.rs`
- Modify: `crates/server/src/main.rs`

- [ ] **Step 1: 实现世界快照 handler**

创建 `crates/server/src/handlers/dashboard/world_snapshot.rs`。handler 用只读事务（`BEGIN READ ONLY` 或 `pool.begin()` + 不 commit），在一次事务内查 agents + world_time + recent_events，保证快照原子可见：
```rust
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;
use std::sync::Arc;

use crate::state::AppState;

#[derive(Serialize)]
pub struct WorldSnapshot {
    pub agents: Vec<serde_json::Value>,
    pub tick_id: i64,
    pub game_day: i64,
    pub world_time: serde_json::Value,
    pub recent_events: Vec<serde_json::Value>,
}

/// GET /api/dashboard/world-snapshot
/// 用只读事务隔离读，消除 tick 边界瞬时跨 agent 不一致。
pub async fn get_world_snapshot(
    State(state): State<Arc<AppState>>,
) -> Result<Json<WorldSnapshot>, StatusCode> {
    let mut tx = state.db_pool.begin().await.map_err(|e| {
        tracing::error!("世界快照 begin tx 失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // 在同一事务内查 agents + world state + recent events
    // 复用 get_all_agents 的 SQL（agents.rs:243-267），但执行在 tx 上
    let agents: Vec<serde_json::Value> = sqlx::query_scalar(
        // 复用 agents.rs 的 LatestStates SQL，结果序列化为 JSON
        r#"WITH LatestStates AS (
            SELECT DISTINCT ON (agent_id) agent_id, node_id, attributes, is_alive, tick_id
            FROM agent_states ORDER BY agent_id, tick_id DESC
        )
        SELECT COALESCE(json_agg(json_build_object(
            'agent_id', a.agent_id, 'name', a.name, 'status', a.status,
            'is_alive', COALESCE(s.is_alive, true),
            'location', COALESCE(s.node_id, 'unknown'),
            'hp', COALESCE((s.attributes->>'hp')::int, 0),
            'attributes', s.attributes
        )), '[]'::json)
        FROM agents a LEFT JOIN LatestStates s ON a.agent_id = s.agent_id"#
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("世界快照 agents 查询失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .unwrap_or_default();

    // 查 tick info（复用 stats.rs 的查询逻辑，或从 game_data 读当前 tick）
    let tick_info: Option<(i64,)> = sqlx::query_as(
        "SELECT MAX(tick_id) FROM tick_logs WHERE status = 'completed'"
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("世界快照 tick 查询失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let tick_id = tick_info.map(|(t,)| t).unwrap_or(0);

    // recent events（最近 20 条有 narrative 的 action log）
    let recent_events: Vec<serde_json::Value> = sqlx::query_scalar(
        r#"SELECT COALESCE(json_agg(json_build_object(
            'agent_id', agent_id, 'action_type', action_type_display,
            'narrative', narrative, 'tick_id', tick_id, 'created_at', created_at
        ) ORDER BY created_at DESC), '[]'::json)
        FROM (SELECT * FROM agent_action_logs WHERE narrative IS NOT NULL ORDER BY created_at DESC LIMIT 20) t"#
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("世界快照 events 查询失败: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .unwrap_or_default();

    // game_day 从 game_data 读
    let gd = state.game_data.get();
    let game_day = gd.game_rules.data.time.game_day; // 确认字段路径

    drop(tx); // 只读事务，drop 即结束

    Ok(Json(WorldSnapshot {
        agents,
        tick_id,
        game_day,
        world_time: serde_json::json!({
            "tick_id": tick_id,
            "game_day": game_day,
        }),
        recent_events,
    }))
}
```

- [ ] **Step 2: 导出 + 注册路由**

`crates/server/src/handlers/mod.rs` 导出新模块。
`crates/server/src/main.rs` 注册 `GET /api/dashboard/world-snapshot`（用 `require_client_read_token`）。

- [ ] **Step 3: 编译验证**

Run: `cargo check --workspace`

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(server): GET /api/dashboard/world-snapshot 统一世界快照（只读事务隔离）"
```

---

### Task 10: 缺口端点（地点/死亡/对话聚合）+ chronicle 幂等 + context 占位符

**Files:**
- Create: `crates/server/migrations/023_chronicle_period_unique.sql`
- Create: `crates/server/src/handlers/dashboard/locations.rs`
- Create: `crates/server/src/handlers/dashboard/deaths.rs`
- Create: `crates/server/src/handlers/dashboard/dialogues.rs`
- Modify: `crates/server/src/handlers/context.rs:105,175`
- Modify: `crates/server/src/main.rs`（路由注册）

- [ ] **Step 1: chronicle 幂等 migration**

创建 `crates/server/migrations/023_chronicle_period_unique.sql`：
```sql
-- 023: chronicle 周期唯一约束（防重算插入重复行）
-- 幂等：与 entrypoint 全量重跑兼容
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'idx_chronicles_period_unique'
    ) THEN
        ALTER TABLE chronicles ADD CONSTRAINT idx_chronicles_period_unique UNIQUE (period_start, period_end);
    END IF;
END $$;
```

- [ ] **Step 2: locations 端点**

创建 `crates/server/src/handlers/dashboard/locations.rs`，从 `state.game_data` 的 LocationRegistry 读节点+边图：
```rust
/// GET /api/dashboard/locations —— 地点/地图图结构
pub async fn get_locations(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let gd = state.game_data.get();
    let registry = &gd.locations.graph; // 确认字段路径
    let nodes: Vec<_> = registry.nodes.values().collect();
    let edges = &registry.edges;
    Json(serde_json::json!({
        "nodes": nodes,
        "edges": edges,
    }))
}
```

- [ ] **Step 3: deaths 端点**

创建 `crates/server/src/handlers/dashboard/deaths.rs`，从 agent_action_logs 查死亡事件时间线。

- [ ] **Step 4: dialogues 聚合端点**

创建 `crates/server/src/handlers/dashboard/dialogues.rs`，从 action_logs 聚合 speak/whisper，按 (agent_a, agent_b) 双向拼接。

- [ ] **Step 5: context.rs 占位符修复**

`crates/server/src/handlers/context.rs:105,175`，把 `"(查看物品详情需要额外查询)"` 改为查真实库存（调 `db::get_agent_inventory`）。

- [ ] **Step 6: 注册路由**

`crates/server/src/main.rs` 注册新端点（均用 `require_client_read_token`）。

- [ ] **Step 7: 编译验证**

Run: `cargo check --workspace`

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(server): 缺口端点（地点/死亡/对话聚合）+ chronicle 幂等 + context 占位符修复"
```

---

## 收尾

### Task 11: 全量验证

- [ ] **Step 1: 全量编译**

Run: `cargo check --workspace --all-targets`
Expected: 0 errors

- [ ] **Step 2: 全量测试**

Run: `cargo test --workspace`
Expected: 全绿

- [ ] **Step 3: 更新 CLAUDE.md（新增端点文档）**

- [ ] **Step 4: 最终 commit**

```bash
git add -A
git commit -m "docs: C 阶段数据可达性完成"
```
