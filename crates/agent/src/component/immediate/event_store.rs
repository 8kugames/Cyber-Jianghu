//! EventStore: SQLite 持久化事件存储层
//!
//! 所有即时事件的状态变更唯一入口。
//! WAL 模式确保写入持久化；所有 SQLite 操作通过 spawn_blocking offload。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rusqlite::params;
use tokio::sync::Notify;
use tracing::{debug, info, warn};
use uuid::Uuid;

use cyber_jianghu_protocol::{EventTriageConfig, EventTriageContext, WorldEventType};

// ============================================================================
// 类型
// ============================================================================

/// DB 中存储的事件行
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub id: i64,
    pub event_id: String,
    pub event_type: WorldEventType,
    pub from_agent_id: Option<String>,
    pub from_agent_name: Option<String>,
    pub description: String,
    pub metadata: String,
    pub received_at_tick: i64,
    pub game_day: i64,
    pub triage_status: String, // pending / urgent / batch / ignored
    pub triage_reason: Option<String>,
    pub triage_batch_id: Option<i64>,
    pub processed_at_tick: Option<i64>,
}

/// Triage 决策（Session LLM 输出）
#[derive(Debug, Clone)]
pub struct TriageDecision {
    pub event_id: String,
    pub decision: String, // urgent / batch / ignored
    pub reason: String,
}

/// 主 tick 消费查询结果
#[derive(Debug, Clone, Default)]
pub struct TriageResult {
    pub urgent: Vec<StoredEvent>,
    pub batch: Vec<StoredEvent>,
}

/// 摄取事件的输入参数
#[derive(Debug, Clone)]
pub struct IncomingEvent {
    pub event_id: Uuid,
    pub event_type: WorldEventType,
    pub description: String,
    pub metadata: serde_json::Value,
    pub from_agent_id: Option<String>,
    pub from_agent_name: Option<String>,
}

// ============================================================================
// EventStore
// ============================================================================

/// SQLite 事件存储（线程安全）
///
/// 内部使用 `Mutex<Connection>` 保护单连接 SQLite。
/// 所有公开方法通过 `tokio::task::spawn_blocking` offload 到阻塞线程池。
pub struct EventStore {
    conn: std::sync::Mutex<rusqlite::Connection>,
    config: EventTriageConfig,
    notify: Arc<Notify>,
}

impl EventStore {
    /// 打开/创建事件数据库
    ///
    /// 启用 WAL 模式 + synchronous=NORMAL，确保持久化性能。
    pub fn open(db_dir: &Path, config: &EventTriageConfig, notify: Arc<Notify>) -> Result<Self> {
        std::fs::create_dir_all(db_dir)
            .with_context(|| format!("创建事件存储目录失败: {:?}", db_dir))?;

        let db_path = db_dir.join("immediate_events.db");
        let conn = rusqlite::Connection::open(&db_path)
            .with_context(|| format!("打开事件数据库失败: {:?}", db_path))?;

        // WAL 模式
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .context("设置 SQLite PRAGMA 失败")?;

        // DDL
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS immediate_events (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id         TEXT    NOT NULL UNIQUE,
                event_type       TEXT    NOT NULL,
                from_agent_id    TEXT,
                from_agent_name  TEXT,
                description      TEXT    NOT NULL,
                metadata         TEXT    DEFAULT '{}',
                received_at_tick INTEGER NOT NULL,
                game_day         INTEGER NOT NULL,
                triage_status    TEXT    NOT NULL DEFAULT 'pending',
                triage_reason    TEXT,
                triage_batch_id  INTEGER,
                processed_at_tick INTEGER,
                created_at       TEXT    DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_events_pending
                ON immediate_events(game_day, triage_status)
                WHERE triage_status = 'pending';

            CREATE INDEX IF NOT EXISTS idx_events_triaged
                ON immediate_events(triage_status, processed_at_tick)
                WHERE processed_at_tick IS NULL AND triage_status IN ('urgent', 'batch');",
        )
        .context("创建事件表失败")?;

        info!(
            "EventStore 已打开: {:?} (WAL 模式)",
            db_path
        );

        Ok(Self {
            conn: std::sync::Mutex::new(conn),
            config: config.clone(),
            notify,
        })
    }

    /// 摄取事件（WebSocket 回调，纯 IO）
    ///
    /// INSERT + Notify 信号。耗时 <1ms。
    pub fn insert_event(
        &self,
        event: &IncomingEvent,
        tick_id: i64,
        game_day: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("SQLite 锁失败: {}", e))?;

        let event_type_str = event.event_type.as_str();
        let metadata_str = serde_json::to_string(&event.metadata).unwrap_or_else(|_| "{}".into());

        conn.execute(
            "INSERT OR IGNORE INTO immediate_events
                (event_id, event_type, from_agent_id, from_agent_name,
                 description, metadata, received_at_tick, game_day)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                event.event_id.to_string(),
                event_type_str,
                event.from_agent_id,
                event.from_agent_name,
                event.description,
                metadata_str,
                tick_id,
                game_day,
            ],
        )
        .with_context(|| format!("插入事件失败: event_id={}", event.event_id))?;

        // 唤醒 Session Triage Engine
        self.notify.notify_one();

        Ok(())
    }

    /// 异步包装：insert_event
    pub async fn insert_event_async(
        self: &Arc<Self>,
        event: &IncomingEvent,
        tick_id: i64,
        game_day: i64,
    ) -> Result<()> {
        let store = self.clone();
        let event = event.clone();
        tokio::task::spawn_blocking(move || store.insert_event(&event, tick_id, game_day))
            .await
            .context("spawn_blocking insert_event 失败")??;
        Ok(())
    }

    /// 查询待 triage 事件（预筛排序 + LIMIT）
    pub fn query_pending(&self, game_day: i64) -> Result<Vec<StoredEvent>> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("SQLite 锁失败: {}", e))?;

        let limit = self.config.pre_filter.max_events_per_triage;
        let priority_map = &self.config.pre_filter.event_type_priority;
        let default_pri = self.config.pre_filter.default_priority;

        // 构建 CASE 表达式用于 ORDER BY
        let case_expr = Self::build_priority_case(priority_map, default_pri);

        let sql = format!(
            "SELECT id, event_id, event_type, from_agent_id, from_agent_name,
                    description, metadata, received_at_tick, game_day,
                    triage_status, triage_reason, triage_batch_id, processed_at_tick
             FROM immediate_events
             WHERE triage_status = 'pending' AND game_day = ?1
             ORDER BY {} DESC
             LIMIT ?2",
            case_expr
        );

        let mut stmt = conn.prepare(&sql).context("准备 query_pending SQL 失败")?;
        let rows = stmt
            .query_map(params![game_day, limit], |row| {
                Ok(StoredEvent {
                    id: row.get(0)?,
                    event_id: row.get(1)?,
                    event_type: {
                        let s: String = row.get(2)?;
                        s.parse().unwrap_or_else(|_| {
                            warn!("未知 event_type '{}'，降级为 SystemNotification", s);
                            WorldEventType::SystemNotification
                        })
                    },
                    from_agent_id: row.get(3)?,
                    from_agent_name: row.get(4)?,
                    description: row.get(5)?,
                    metadata: row.get(6)?,
                    received_at_tick: row.get(7)?,
                    game_day: row.get(8)?,
                    triage_status: row.get(9)?,
                    triage_reason: row.get(10)?,
                    triage_batch_id: row.get(11)?,
                    processed_at_tick: row.get(12)?,
                })
            })
            .context("执行 query_pending 失败")?;

        let mut events = Vec::new();
        for row in rows {
            match row {
                Ok(e) => events.push(e),
                Err(e) => warn!("解析 pending event 行失败: {}", e),
            }
        }
        Ok(events)
    }

    /// 异步包装：query_pending
    pub async fn query_pending_async(self: &Arc<Self>, game_day: i64) -> Result<Vec<StoredEvent>> {
        let store = self.clone();
        let result = tokio::task::spawn_blocking(move || store.query_pending(game_day))
            .await
            .context("spawn_blocking query_pending 失败")??;
        Ok(result)
    }

    /// 写入 triage 决策（批量）
    pub fn update_triage(&self, decisions: &[TriageDecision], batch_id: i64) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("SQLite 锁失败: {}", e))?;

        for dec in decisions {
            conn.execute(
                "UPDATE immediate_events
                 SET triage_status = ?1, triage_reason = ?2, triage_batch_id = ?3
                 WHERE event_id = ?4",
                params![dec.decision, dec.reason, batch_id, dec.event_id],
            )
            .with_context(|| format!("更新 triage 决策失败: event_id={}", dec.event_id))?;
        }

        debug!(
            "已写入 {} 条 triage 决策 (batch_id={})",
            decisions.len(),
            batch_id
        );
        Ok(())
    }

    /// 异步包装：update_triage
    pub async fn update_triage_async(
        self: &Arc<Self>,
        decisions: Vec<TriageDecision>,
        batch_id: i64,
    ) -> Result<()> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.update_triage(&decisions, batch_id))
            .await
            .context("spawn_blocking update_triage 失败")??;
        Ok(())
    }

    /// 查询已 triage 未消费事件（主 tick 用）
    pub fn query_triaged(&self, context_config: &EventTriageContext) -> Result<TriageResult> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("SQLite 锁失败: {}", e))?;

        // URGENT: 逐条，top-N
        let mut urgent_stmt = conn.prepare(
            "SELECT id, event_id, event_type, from_agent_id, from_agent_name,
                    description, metadata, received_at_tick, game_day,
                    triage_status, triage_reason, triage_batch_id, processed_at_tick
             FROM immediate_events
             WHERE triage_status = 'urgent' AND processed_at_tick IS NULL
             ORDER BY received_at_tick DESC
             LIMIT ?1",
        )?;

        let urgent_rows = urgent_stmt.query_map(params![context_config.max_urgent_events], |row| {
            Self::row_to_stored_event(row)
        })?;

        let mut urgent = Vec::new();
        for row in urgent_rows {
            match row {
                Ok(e) => urgent.push(e),
                Err(e) => warn!("解析 urgent event 行失败: {}", e),
            }
        }

        // BATCH: 全部（主 tick 会做摘要）
        let mut batch_stmt = conn.prepare(
            "SELECT id, event_id, event_type, from_agent_id, from_agent_name,
                    description, metadata, received_at_tick, game_day,
                    triage_status, triage_reason, triage_batch_id, processed_at_tick
             FROM immediate_events
             WHERE triage_status = 'batch' AND processed_at_tick IS NULL
             ORDER BY received_at_tick DESC",
        )?;

        let batch_rows = batch_stmt.query_map([], |row| {
            Self::row_to_stored_event(row)
        })?;

        let mut batch = Vec::new();
        for row in batch_rows {
            match row {
                Ok(e) => batch.push(e),
                Err(e) => warn!("解析 batch event 行失败: {}", e),
            }
        }

        Ok(TriageResult { urgent, batch })
    }

    /// 异步包装：query_triaged
    pub async fn query_triaged_async(
        self: &Arc<Self>,
        context_config: EventTriageContext,
    ) -> Result<TriageResult> {
        let store = self.clone();
        let result = tokio::task::spawn_blocking(move || store.query_triaged(&context_config))
            .await
            .context("spawn_blocking query_triaged 失败")??;
        Ok(result)
    }

    /// 按事件 ID 标记已消费（避免与后台 triage 批次竞态）
    pub fn mark_processed_by_ids(&self, ids: &[i64], tick_id: i64) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("SQLite 锁失败: {}", e))?;

        // 构建 IN 子句：?, ?, ?
        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "UPDATE immediate_events
             SET processed_at_tick = ?
             WHERE id IN ({}) AND processed_at_tick IS NULL",
            placeholders.join(", ")
        );

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> =
            ids.iter().map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>).collect();
        params_vec.push(Box::new(tick_id));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let affected = conn.execute(&sql, param_refs.as_slice())
            .context("按 ID 标记已消费事件失败")?;

        if affected > 0 {
            debug!("已标记 {} 条事件为已消费 (tick_id={})", affected, tick_id);
        }
        Ok(())
    }

    /// 异步包装：mark_processed_by_ids
    pub async fn mark_processed_by_ids_async(
        self: &Arc<Self>,
        ids: Vec<i64>,
        tick_id: i64,
    ) -> Result<()> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.mark_processed_by_ids(&ids, tick_id))
            .await
            .context("spawn_blocking mark_processed_by_ids 失败")??;
        Ok(())
    }

    /// 清理过期事件
    pub fn cleanup_old(&self, current_game_day: i64) -> Result<()> {
        let retention = self.config.retention_game_days as i64;
        let cutoff = current_game_day.saturating_sub(retention);

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("SQLite 锁失败: {}", e))?;
        let affected = conn
            .execute(
                "DELETE FROM immediate_events WHERE game_day < ?1",
                params![cutoff],
            )
            .context("清理过期事件失败")?;

        if affected > 0 {
            info!(
                "已清理 {} 条过期事件 (game_day < {})",
                affected, cutoff
            );
        }
        Ok(())
    }

    /// 异步包装：cleanup_old
    pub async fn cleanup_old_async(self: &Arc<Self>, current_game_day: i64) -> Result<()> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.cleanup_old(current_game_day))
            .await
            .context("spawn_blocking cleanup_old 失败")??;
        Ok(())
    }

    /// 获取 Notify 引用（供外部组件唤醒 Session Triage）
    pub fn notify(&self) -> &Arc<Notify> {
        &self.notify
    }

    /// 获取配置引用
    pub fn config(&self) -> &EventTriageConfig {
        &self.config
    }

    // ---- 内部辅助 ----

    fn row_to_stored_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
        let event_type_str: String = row.get(2)?;
        let event_type = event_type_str.parse().unwrap_or_else(|_| {
            warn!("未知 event_type '{}'，降级为 SystemNotification", event_type_str);
            WorldEventType::SystemNotification
        });
        Ok(StoredEvent {
            id: row.get(0)?,
            event_id: row.get(1)?,
            event_type,
            from_agent_id: row.get(3)?,
            from_agent_name: row.get(4)?,
            description: row.get(5)?,
            metadata: row.get(6)?,
            received_at_tick: row.get(7)?,
            game_day: row.get(8)?,
            triage_status: row.get(9)?,
            triage_reason: row.get(10)?,
            triage_batch_id: row.get(11)?,
            processed_at_tick: row.get(12)?,
        })
    }

    /// 构建 SQL CASE 表达式用于 priority 排序
    fn build_priority_case(
        priorities: &HashMap<WorldEventType, i32>,
        default: i32,
    ) -> String {
        let mut cases: Vec<String> = priorities
            .iter()
            .map(|(et, pri)| format!("WHEN '{}' THEN {}", et.as_str(), pri))
            .collect();
        cases.sort_by(|a, b| b.cmp(a)); // 降序排列保证确定性

        if cases.is_empty() {
            return default.to_string();
        }

        format!("CASE event_type {} ELSE {} END", cases.join(" "), default)
    }
}
