// ============================================================================
// Intent History Store - Intent 历史存储（SQLite 持久化）
// ============================================================================
//
// 用于存储每个 Tick 的 Intent 提交记录，支持经历日志查询。
// 数据来源：
// - thought_log: Agent 提交 Intent 时的思考日志
// - observer_thought: Observer Agent 审查时的思维链
// - event: WorldState.events_log 中的事件描述
//
// 存储后端：SQLite，按 agent_id 隔离（per-agent 数据库文件）

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Observer 反思之魂的审查意见
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ObserverThought {
    /// 审查结果（approved/rejected）
    pub result: String,
    /// 审查原因
    pub reason: String,
    /// 叙事化描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrative: Option<String>,
}

/// Intent 历史条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IntentHistoryEntry {
    /// Tick ID
    pub tick_id: i64,
    /// Intent ID
    pub intent_id: Uuid,
    /// 动作类型
    pub action_type: String,
    /// Agent 思考日志（intent_summary 的来源）
    pub thought_log: Option<String>,
    /// Observer 思维链（结构化）
    pub observer_thought: Option<ObserverThought>,
    /// 事件描述（来自 WorldState.events_log）
    pub event: Option<String>,
    /// 世界时间（来自 WorldState.world_time）
    pub world_time: Option<String>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
}

/// Intent 历史存储（SQLite 持久化）
///
/// 按 agent_id 隔离，使用独立的 SQLite 文件。
#[derive(Debug, Clone)]
pub struct IntentHistoryStore {
    /// 当前 Agent ID
    #[allow(dead_code)]
    agent_id: Uuid,
    /// 数据库连接
    conn: Arc<Mutex<Connection>>,
    /// 数据库路径
    #[allow(dead_code)]
    db_path: PathBuf,
}

impl IntentHistoryStore {
    /// 打开或创建 Intent 历史存储
    pub fn open(agent_id: Uuid, db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create database directory")?;
        }

        let conn = Connection::open(db_path).context("Failed to open intent history database")?;
        Self::init_schema(&conn)?;

        Ok(Self {
            agent_id,
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.to_path_buf(),
        })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS intent_history (
                tick_id INTEGER PRIMARY KEY,
                intent_id TEXT NOT NULL,
                action_type TEXT NOT NULL DEFAULT '',
                thought_log TEXT,
                observer_thought TEXT,
                event TEXT,
                world_time TEXT,
                created_at TEXT NOT NULL
            )",
            [],
        )
        .context("Failed to create intent_history table")?;

        conn.execute("PRAGMA journal_mode = WAL", []).ok();
        conn.execute("PRAGMA synchronous = NORMAL", []).ok();

        Ok(())
    }

    /// 记录 Intent 提交
    pub async fn record_intent(
        &self,
        tick_id: i64,
        intent_id: Uuid,
        action_type: String,
        thought_log: Option<String>,
        world_time: Option<String>,
    ) {
        let conn = self.conn.lock().expect("intent_history lock not poisoned");
        let created_at = Utc::now().to_rfc3339();

        let result = conn.execute(
            "INSERT INTO intent_history
             (tick_id, intent_id, action_type, thought_log, world_time, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(tick_id) DO UPDATE SET
                intent_id = excluded.intent_id,
                action_type = excluded.action_type,
                thought_log = excluded.thought_log,
                world_time = COALESCE(excluded.world_time, world_time),
                created_at = excluded.created_at",
            params![
                tick_id,
                intent_id.to_string(),
                action_type,
                thought_log,
                world_time,
                created_at
            ],
        );

        match result {
            Ok(_) => tracing::debug!("[intent_history] Recorded intent for tick {}", tick_id),
            Err(e) => tracing::warn!(
                "[intent_history] Failed to record intent for tick {}: {}",
                tick_id,
                e
            ),
        }
    }

    /// 记录事件（来自 WorldState.events_log）
    pub async fn record_event(&self, tick_id: i64, event: &str, world_time: Option<String>) {
        let conn = self.conn.lock().expect("intent_history lock not poisoned");

        let tx = match conn.unchecked_transaction() {
            Ok(tx) => tx,
            Err(e) => {
                tracing::warn!("[intent_history] Failed to begin transaction: {}", e);
                return;
            }
        };

        let updated = tx
            .execute(
                "UPDATE intent_history SET event = ?1, world_time = ?2 WHERE tick_id = ?3",
                params![event, world_time, tick_id],
            )
            .unwrap_or(0);

        if updated == 0 {
            let created_at = Utc::now().to_rfc3339();
            let _ = tx.execute(
                "INSERT OR IGNORE INTO intent_history
                 (tick_id, intent_id, action_type, event, world_time, created_at)
                 VALUES (?1, ?2, '', ?3, ?4, ?5)",
                params![
                    tick_id,
                    Uuid::nil().to_string(),
                    event,
                    world_time,
                    created_at
                ],
            );
        }

        if tx.commit().is_err() {
            tracing::warn!(
                "[intent_history] Failed to commit event for tick {}",
                tick_id
            );
        }
    }

    /// 更新 Observer 思维链（Upsert 模式）
    ///
    /// ReflectorSoul 审查可能在 ActorSoul 记录 intent 之前完成，
    /// 因此使用 upsert：先尝试 UPDATE，若行不存在则 INSERT 占位行，
    /// 等 `record_intent()` 调用时再补全字段。
    pub async fn update_observer_thought(&self, tick_id: i64, thought: ObserverThought) {
        let conn = self.conn.lock().expect("intent_history lock not poisoned");
        let json = serde_json::to_string(&thought).unwrap_or_default();

        let updated = conn
            .execute(
                "UPDATE intent_history SET observer_thought = ?1 WHERE tick_id = ?2",
                params![json, tick_id],
            )
            .unwrap_or(0);

        if updated == 0 {
            let created_at = Utc::now().to_rfc3339();
            let _ = conn.execute(
                "INSERT OR IGNORE INTO intent_history
                 (tick_id, intent_id, action_type, observer_thought, created_at)
                 VALUES (?1, ?2, '', ?3, ?4)",
                params![tick_id, Uuid::nil().to_string(), json, created_at],
            );
            tracing::debug!(
                "[intent_history] Inserted stub for tick {} (observer thought arrived first)",
                tick_id
            );
        } else {
            tracing::debug!(
                "[intent_history] Updated observer thought for tick {}",
                tick_id
            );
        }
    }

    /// 获取最近的行动历史（用于认知引擎记忆上下文）
    pub async fn get_recent_history(&self, limit: usize) -> Vec<IntentHistoryEntry> {
        let conn = match self.conn.lock() {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!("[intent_history] Lock poisoned: {}", e);
                return Vec::new();
            }
        };

        let mut stmt = match conn.prepare_cached(
            "SELECT tick_id, intent_id, action_type, thought_log, observer_thought, event, world_time, created_at
             FROM intent_history
             ORDER BY tick_id DESC
             LIMIT ?1",
        ) {
            Ok(stmt) => stmt,
            Err(e) => {
                tracing::warn!("[intent_history] Failed to prepare query: {}", e);
                return Vec::new();
            }
        };

        match stmt.query_map(params![limit as i64], |row| {
            Ok(IntentHistoryEntry {
                tick_id: row.get(0)?,
                intent_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                action_type: row.get(2)?,
                thought_log: row.get(3)?,
                observer_thought: row.get::<_, Option<String>>(4)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                event: row.get(5)?,
                world_time: row.get(6)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            })
        }) {
            Ok(rows) => rows.filter_map(Result::ok).collect(),
            Err(e) => {
                tracing::warn!("[intent_history] Failed to query recent history: {}", e);
                Vec::new()
            }
        }
    }

    /// 获取指定 tick 的条目
    pub async fn get_by_tick(&self, tick_id: i64) -> Option<IntentHistoryEntry> {
        let conn = self.conn.lock().ok()?;
        Self::query_one(&conn, tick_id)
    }

    /// 分页获取所有条目（按 tick_id 降序）
    pub async fn get_page(&self, page: u32, limit: u32) -> Result<(Vec<IntentHistoryEntry>, u32)> {
        let page = page.max(1);
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock failed: {}", e))?;

        let total: u32 = conn
            .query_row("SELECT COUNT(*) FROM intent_history", [], |row| row.get(0))
            .unwrap_or(0);

        let offset = ((page - 1) * limit) as i64;
        let mut stmt = conn.prepare(
            "SELECT tick_id, intent_id, action_type, thought_log, observer_thought,
                    event, world_time, created_at
             FROM intent_history
             ORDER BY tick_id DESC
             LIMIT ?1 OFFSET ?2",
        )?;

        let entries = stmt
            .query_map(params![limit, offset], |row| Ok(Self::row_to_entry(row)))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((entries, total))
    }

    fn query_one(conn: &Connection, tick_id: i64) -> Option<IntentHistoryEntry> {
        let mut stmt = conn
            .prepare(
                "SELECT tick_id, intent_id, action_type, thought_log, observer_thought,
                        event, world_time, created_at
                 FROM intent_history WHERE tick_id = ?",
            )
            .ok()?;

        stmt.query_row(params![tick_id], |row| Ok(Self::row_to_entry(row)))
            .ok()
    }

    fn row_to_entry(row: &rusqlite::Row<'_>) -> IntentHistoryEntry {
        let created_at_str: String = row.get(7).unwrap_or_default();
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        let observer_thought: Option<ObserverThought> = row
            .get::<_, String>(4)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());

        IntentHistoryEntry {
            tick_id: row.get(0).unwrap_or(0),
            intent_id: row
                .get::<_, String>(1)
                .ok()
                .and_then(|s| Uuid::parse_str(&s).ok())
                .unwrap_or(Uuid::nil()),
            action_type: row.get(2).unwrap_or_default(),
            thought_log: row.get(3).ok(),
            observer_thought,
            event: row.get(5).ok(),
            world_time: row.get(6).ok(),
            created_at,
        }
    }

    /// 获取当前条目数量
    pub async fn len(&self) -> usize {
        self.conn
            .lock()
            .ok()
            .and_then(|conn| {
                conn.query_row("SELECT COUNT(*) FROM intent_history", [], |row| {
                    row.get::<_, usize>(0)
                })
                .ok()
            })
            .unwrap_or(0)
    }

    /// 检查是否为空
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// 清空所有记录
    pub fn clear_all(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Lock failed: {}", e))?;
        conn.execute("DELETE FROM intent_history", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_store() -> (TempDir, IntentHistoryStore) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("intent_history.db");
        let store = IntentHistoryStore::open(Uuid::new_v4(), &db_path).unwrap();
        (temp_dir, store)
    }

    #[tokio::test]
    async fn test_record_and_get_intent() {
        let (_dir, store) = make_store();
        let intent_id = Uuid::new_v4();

        store
            .record_intent(
                1,
                intent_id,
                "idle".to_string(),
                Some("思考中...".to_string()),
                None,
            )
            .await;

        let entry = store.get_by_tick(1).await;
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.tick_id, 1);
        assert_eq!(entry.action_type, "idle");
        assert_eq!(entry.thought_log, Some("思考中...".to_string()));
        assert!(entry.observer_thought.is_none());
    }

    #[tokio::test]
    async fn test_update_observer_thought() {
        let (_dir, store) = make_store();
        let intent_id = Uuid::new_v4();

        store
            .record_intent(1, intent_id, "idle".to_string(), None, None)
            .await;

        store
            .update_observer_thought(
                1,
                ObserverThought {
                    result: "approved".to_string(),
                    reason: "这个行为符合人设".to_string(),
                    narrative: Some("在江湖中行走，乐于助人是美德".to_string()),
                },
            )
            .await;

        let entry = store.get_by_tick(1).await.unwrap();
        assert!(entry.observer_thought.is_some());
        let ot = entry.observer_thought.unwrap();
        assert_eq!(ot.result, "approved");
        assert_eq!(ot.reason, "这个行为符合人设");
        assert_eq!(
            ot.narrative,
            Some("在江湖中行走，乐于助人是美德".to_string())
        );
    }

    #[tokio::test]
    async fn test_record_event() {
        let (_dir, store) = make_store();
        let intent_id = Uuid::new_v4();

        store
            .record_intent(
                5,
                intent_id,
                "speak".to_string(),
                Some("想说点什么".to_string()),
                None,
            )
            .await;

        store
            .record_event(5, "张三在广场上大声说话", Some("第三天 申时".to_string()))
            .await;

        let entry = store.get_by_tick(5).await.unwrap();
        assert_eq!(entry.event, Some("张三在广场上大声说话".to_string()));
        assert_eq!(entry.world_time, Some("第三天 申时".to_string()));
    }

    #[tokio::test]
    async fn test_record_event_without_intent() {
        let (_dir, store) = make_store();

        store.record_event(10, "风吹过广场", None).await;

        let entry = store.get_by_tick(10).await.unwrap();
        assert_eq!(entry.event, Some("风吹过广场".to_string()));
        assert!(entry.thought_log.is_none());
    }

    #[tokio::test]
    async fn test_get_page() {
        let (_dir, store) = make_store();

        for i in 1..=10 {
            store
                .record_intent(
                    i,
                    Uuid::new_v4(),
                    "idle".to_string(),
                    Some(format!("thought {}", i)),
                    None,
                )
                .await;
        }

        let (page1, total) = store.get_page(1, 3).await.unwrap();
        assert_eq!(total, 10);
        assert_eq!(page1.len(), 3);
        // 降序排列，最新的在前
        assert_eq!(page1[0].tick_id, 10);
        assert_eq!(page1[1].tick_id, 9);
        assert_eq!(page1[2].tick_id, 8);

        let (page2, _) = store.get_page(2, 3).await.unwrap();
        assert_eq!(page2[0].tick_id, 7);
    }

    #[tokio::test]
    async fn test_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("intent_history.db");
        let agent_id = Uuid::new_v4();
        let intent_id = Uuid::new_v4();

        // 写入
        let store = IntentHistoryStore::open(agent_id, &db_path).unwrap();
        store
            .record_intent(
                42,
                intent_id,
                "move".to_string(),
                Some("去集市".to_string()),
                None,
            )
            .await;
        drop(store);

        // 重新打开，数据应保留
        let store2 = IntentHistoryStore::open(agent_id, &db_path).unwrap();
        let entry = store2.get_by_tick(42).await.unwrap();
        assert_eq!(entry.action_type, "move");
        assert_eq!(entry.thought_log, Some("去集市".to_string()));
    }
}
