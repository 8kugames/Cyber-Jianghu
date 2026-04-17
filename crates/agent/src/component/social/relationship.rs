// ============================================================================
// 关系记忆存储层
// ============================================================================
//
// 实现关系记忆的持久化存储
// 支持对其他 Agent 的关系记忆的长期保存和查询
// ============================================================================

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::component::social::relationship_types::{KeyEvent, RelationshipMemory};

/// 关系记忆存储
///
/// 使用 SQLite 存储对其他 Agent 的关系记忆
///
/// # Thread Safety
///
/// 此实现使用 `Arc<Mutex<Connection>>` 提供线程安全性。
/// `rusqlite::Connection` 是 `Send` 但不是 `Sync`，
/// 因此必须使用 `Mutex` 而不是 `RwLock` 来保证线程安全。
///
/// 注意：虽然此结构可以在线程间传递，但 SQLite 本身不支持
/// 多线程并发写入。在高并发场景下，建议使用连接池（如 r2d2）。
#[derive(Clone)]
pub struct RelationshipStore {
    /// 当前 Agent ID
    #[allow(dead_code)]
    agent_id: Uuid,
    /// 数据库连接（使用 Mutex 保证线程安全）
    /// rusqlite::Connection 是 Send 但不是 Sync
    conn: Arc<Mutex<Connection>>,
    /// 数据库路径
    #[allow(dead_code)]
    db_path: PathBuf,
    /// 最大事件数量
    #[allow(dead_code)]
    max_events: usize,
}

impl RelationshipStore {
    /// 打开或创建关系记忆存储
    ///
    /// 如果数据库不存在，将自动创建
    pub fn open(agent_id: Uuid, db_path: &Path) -> Result<Self> {
        // 确保父目录存在
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create database directory")?;
        }

        // 打开数据库连接
        let conn = Connection::open(db_path).context("Failed to open database")?;

        // 初始化数据库结构
        Self::init_schema(&conn)?;

        Ok(Self {
            agent_id,
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.to_path_buf(),
            max_events: 20,
        })
    }

    /// 初始化数据库结构
    fn init_schema(conn: &Connection) -> Result<()> {
        // 创建关系表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS relationships (
                target_agent_id TEXT PRIMARY KEY,
                target_name TEXT NOT NULL,
                favorability INTEGER DEFAULT 0 CHECK(favorability >= -100 AND favorability <= 100),
                last_interaction_tick INTEGER DEFAULT 0,
                updated_at TIMESTAMP NOT NULL,
                self_description TEXT DEFAULT '',
                description_tick INTEGER DEFAULT 0
            )",
            [],
        )
        .context("Failed to create relationships table")?;

        // 兼容旧数据库：检查并添加新字段
        // 使用 PRAGMA table_info 检查列是否存在
        let has_self_description: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('relationships') WHERE name = 'self_description'",
                [],
                |row| row.get::<_, i32>(0).map(|c| c > 0),
            )
            .unwrap_or(false);

        if !has_self_description {
            conn.execute(
                "ALTER TABLE relationships ADD COLUMN self_description TEXT DEFAULT ''",
                [],
            )
            .ok();
        }

        let has_description_tick: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('relationships') WHERE name = 'description_tick'",
                [],
                |row| row.get::<_, i32>(0).map(|c| c > 0),
            )
            .unwrap_or(false);

        if !has_description_tick {
            conn.execute(
                "ALTER TABLE relationships ADD COLUMN description_tick INTEGER DEFAULT 0",
                [],
            )
            .ok();
        }

        // 创建关键事件表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS key_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target_agent_id TEXT NOT NULL,
                tick_id INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                description TEXT NOT NULL,
                favorability_delta INTEGER NOT NULL,
                timestamp TEXT NOT NULL,
                FOREIGN KEY (target_agent_id) REFERENCES relationships(target_agent_id) ON DELETE CASCADE
            )",
            [],
        ).context("Failed to create key_events table")?;

        // 创建索引
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_key_events_target
             ON key_events(target_agent_id)",
            [],
        )
        .ok();

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_key_events_tick
             ON key_events(tick_id DESC)",
            [],
        )
        .ok();

        // 性能优化
        conn.execute("PRAGMA journal_mode = WAL", []).ok();
        conn.execute("PRAGMA synchronous = NORMAL", []).ok();
        conn.execute("PRAGMA cache_size = -32000", []).ok(); // 32MB cache

        Ok(())
    }

    /// 获取对某个目标 Agent 的关系记忆
    pub fn get_relationship(&self, target_agent_id: Uuid) -> Result<Option<RelationshipMemory>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;

        // 查询关系基本信息
        let mut stmt = conn.prepare(
            "SELECT target_name, favorability, last_interaction_tick, updated_at,
                     self_description, description_tick
             FROM relationships
             WHERE target_agent_id = ?",
        )?;

        let relationship_result = stmt.query_row(params![target_agent_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        });

        let (
            target_name,
            favorability,
            last_interaction_tick,
            updated_at_str,
            self_description,
            description_tick,
        ) = match relationship_result {
            Ok(data) => data,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        // 解析时间戳（如果解析失败，使用当前时间作为默认值）
        let updated_at = updated_at_str
            .parse::<DateTime<Utc>>()
            .with_context(|| format!("Failed to parse updated_at timestamp: {}", updated_at_str))
            .unwrap_or_else(|_| Utc::now());

        // 查询关键事件
        let mut stmt = conn.prepare(
            "SELECT tick_id, event_type, description, favorability_delta, timestamp
             FROM key_events
             WHERE target_agent_id = ?
             ORDER BY tick_id DESC",
        )?;

        let key_events = stmt
            .query_map(params![target_agent_id.to_string()], |row| {
                let timestamp_str: String = row.get(4)?;
                let timestamp = timestamp_str
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now());
                Ok(KeyEvent {
                    tick_id: row.get(0)?,
                    event_type: row.get(1)?,
                    description: row.get(2)?,
                    favorability_delta: row.get(3)?,
                    timestamp,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(RelationshipMemory {
            target_agent_id,
            target_name,
            favorability,
            key_events,
            last_interaction_tick,
            updated_at,
            self_description,
            description_tick,
        }))
    }

    /// 创建或更新关系记忆
    pub fn upsert_relationship(&self, memory: &RelationshipMemory) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;

        // 开启事务
        let transaction = conn.unchecked_transaction()?;

        // 更新或插入关系基本信息
        transaction
            .execute(
                "INSERT OR REPLACE INTO relationships
             (target_agent_id, target_name, favorability, last_interaction_tick, updated_at,
              self_description, description_tick)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    memory.target_agent_id.to_string(),
                    memory.target_name,
                    memory.favorability,
                    memory.last_interaction_tick,
                    memory.updated_at.to_rfc3339(),
                    memory.self_description,
                    memory.description_tick,
                ],
            )
            .context("Failed to upsert relationship")?;

        // 删除旧的事件（因为我们使用 INSERT OR REPLACE，需要先删除关联的事件）
        transaction
            .execute(
                "DELETE FROM key_events WHERE target_agent_id = ?",
                params![memory.target_agent_id.to_string()],
            )
            .ok();

        // 插入所有事件
        for event in &memory.key_events {
            transaction
                .execute(
                    "INSERT INTO key_events
                 (target_agent_id, tick_id, event_type, description, favorability_delta, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        memory.target_agent_id.to_string(),
                        event.tick_id,
                        event.event_type,
                        event.description,
                        event.favorability_delta,
                        event.timestamp.to_rfc3339(),
                    ],
                )
                .context("Failed to insert key event")?;
        }

        // 提交事务
        transaction
            .commit()
            .context("Failed to commit transaction")?;

        Ok(())
    }

    /// 获取所有关系记忆
    pub fn get_all_relationships(&self) -> Result<Vec<RelationshipMemory>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;

        // 查询所有关系的基本信息
        let mut stmt = conn.prepare(
            "SELECT target_agent_id, target_name, favorability, last_interaction_tick, updated_at,
                     self_description, description_tick
             FROM relationships
             ORDER BY updated_at DESC",
        )?;

        let relationships = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut result = Vec::new();

        for (
            target_agent_id_str,
            target_name,
            favorability,
            last_interaction_tick,
            updated_at_str,
            self_description,
            description_tick,
        ) in relationships
        {
            // 解析目标 Agent ID
            let target_agent_id = Uuid::parse_str(&target_agent_id_str)
                .map_err(|e| anyhow::anyhow!("Invalid UUID: {}", e))?;

            // 解析时间戳（如果解析失败，使用当前时间作为默认值）
            let updated_at = updated_at_str
                .parse::<DateTime<Utc>>()
                .with_context(|| {
                    format!("Failed to parse updated_at timestamp: {}", updated_at_str)
                })
                .unwrap_or_else(|_| Utc::now());

            // 查询关键事件
            let mut stmt = conn.prepare(
                "SELECT tick_id, event_type, description, favorability_delta, timestamp
                 FROM key_events
                 WHERE target_agent_id = ?
                 ORDER BY tick_id DESC",
            )?;

            let key_events = stmt
                .query_map(params![target_agent_id_str], |row| {
                    let timestamp_str: String = row.get(4)?;
                    let timestamp = timestamp_str
                        .parse::<DateTime<Utc>>()
                        .unwrap_or_else(|_| Utc::now());
                    Ok(KeyEvent {
                        tick_id: row.get(0)?,
                        event_type: row.get(1)?,
                        description: row.get(2)?,
                        favorability_delta: row.get(3)?,
                        timestamp,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;

            result.push(RelationshipMemory {
                target_agent_id,
                target_name,
                favorability,
                key_events,
                last_interaction_tick,
                updated_at,
                self_description,
                description_tick,
            });
        }

        Ok(result)
    }

    /// 删除关系记忆
    pub fn delete_relationship(&self, target_agent_id: Uuid) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;

        conn.execute(
            "DELETE FROM relationships WHERE target_agent_id = ?",
            params![target_agent_id.to_string()],
        )?;

        // 关键事件会通过 ON DELETE CASCADE 自动删除

        Ok(())
    }

    /// 获取关系数量
    pub fn count(&self) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;

        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM relationships", [], |row| row.get(0))?;

        Ok(count as usize)
    }

    /// 获取数据库路径
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// 更新自我描述字段
    pub fn update_self_description(
        &self,
        target_id: Uuid,
        description: &str,
        tick: i64,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;

        conn.execute(
            "UPDATE relationships
             SET self_description = ?1, description_tick = ?2
             WHERE target_agent_id = ?3",
            params![description, tick, target_id.to_string()],
        )
        .context("Failed to update self_description")?;

        Ok(())
    }

    /// 记录社交事件并更新好感度
    ///
    /// delta 由外部（LLM 评估）决定，此处只负责持久化。
    pub fn record_social_event(
        &self,
        target_agent_id: Uuid,
        target_name: &str,
        tick_id: i64,
        action: &str,
        description: &str,
        delta: i32,
    ) -> Result<()> {

        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;

        let transaction = conn.unchecked_transaction()?;

        // 读取现有好感度（不存在则为 0）
        let existing_favorability: i32 = transaction
            .query_row(
                "SELECT favorability FROM relationships WHERE target_agent_id = ?",
                params![target_agent_id.to_string()],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let new_favorability = (existing_favorability + delta).clamp(-100, 100);
        let now = chrono::Utc::now().to_rfc3339();

        // 使用 UPDATE 而非 INSERT OR REPLACE，避免 CASCADE 删除 key_events
        let updated = transaction.execute(
            "UPDATE relationships
             SET target_name = ?2, favorability = ?3, last_interaction_tick = ?4, updated_at = ?5
             WHERE target_agent_id = ?1",
            params![
                target_agent_id.to_string(),
                target_name,
                new_favorability,
                tick_id,
                now,
            ],
        )?;

        // 如果不存在则 INSERT（不会触发 CASCADE）
        if updated == 0 {
            transaction.execute(
                "INSERT INTO relationships
                 (target_agent_id, target_name, favorability, last_interaction_tick, updated_at,
                  self_description, description_tick)
                 VALUES (?1, ?2, ?3, ?4, ?5, '', 0)",
                params![
                    target_agent_id.to_string(),
                    target_name,
                    new_favorability,
                    tick_id,
                    now,
                ],
            )?;
        }

        // 插入 key event
        transaction.execute(
            "INSERT INTO key_events
             (target_agent_id, tick_id, event_type, description, favorability_delta, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                target_agent_id.to_string(),
                tick_id,
                action,
                description,
                delta,
                now,
            ],
        )?;

        // 清理：只保留最近 max_events 条事件
        transaction.execute(
            "DELETE FROM key_events WHERE target_agent_id = ?1 AND id NOT IN \
             (SELECT id FROM key_events WHERE target_agent_id = ?1 ORDER BY tick_id DESC LIMIT ?2)",
            params![target_agent_id.to_string(), self.max_events as i64],
        )?;

        transaction.commit()?;

        tracing::debug!(
            "社交事件记录: {} -> {} (action={}, delta={}, new_fav={})",
            self.agent_id, target_name, action, delta, new_favorability
        );

        Ok(())
    }

    /// 清空所有关系记忆
    pub fn clear_all(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;

        conn.execute("DELETE FROM relationships", [])?;
        // 关键事件会通过 ON DELETE CASCADE 自动删除

        Ok(())
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_and_get_relationship() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();

        // 创建关系记忆
        let mut memory = RelationshipMemory::new(target_id, "张三");
        memory.update_favorability(50);
        memory.update_interaction(10);

        // 保存
        store.upsert_relationship(&memory).unwrap();

        // 读取
        let retrieved = store.get_relationship(target_id).unwrap().unwrap();
        assert_eq!(retrieved.target_name, "张三");
        assert_eq!(retrieved.favorability, 50);
        assert_eq!(retrieved.last_interaction_tick, 10);
    }

    #[test]
    fn test_update_relationship() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();

        // 创建初始关系
        let mut memory = RelationshipMemory::new(target_id, "张三");
        memory.update_favorability(30);
        store.upsert_relationship(&memory).unwrap();

        // 更新关系
        memory.update_favorability(20);
        memory.add_event(KeyEvent::new(5, "对话", "聊得很开心", 10));
        store.upsert_relationship(&memory).unwrap();

        // 验证更新
        let retrieved = store.get_relationship(target_id).unwrap().unwrap();
        assert_eq!(retrieved.favorability, 50); // 30 + 20
        assert_eq!(retrieved.key_events.len(), 1);
        assert_eq!(retrieved.key_events[0].description, "聊得很开心");
    }

    #[test]
    fn test_get_all_relationships() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let agent_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();

        // 添加多个关系
        for i in 1..=3 {
            let memory = RelationshipMemory::new(Uuid::new_v4(), format!("Agent{}", i));
            store.upsert_relationship(&memory).unwrap();
        }

        // 获取所有关系
        let all = store.get_all_relationships().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_delete_relationship() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();

        // 创建关系
        let memory = RelationshipMemory::new(target_id, "张三");
        store.upsert_relationship(&memory).unwrap();

        // 删除关系
        store.delete_relationship(target_id).unwrap();

        // 验证删除
        let retrieved = store.get_relationship(target_id).unwrap();
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_record_social_event_new_target() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();

        // 记录社交事件（新目标）
        store.record_social_event(
            target_id,
            "李四",
            10,
            "give",
            "李四给了你一个馒头",
            5,
        ).unwrap();

        // 验证关系自动创建
        let rel = store.get_relationship(target_id).unwrap().unwrap();
        assert_eq!(rel.target_name, "李四");
        assert_eq!(rel.favorability, 5);
        assert_eq!(rel.key_events.len(), 1);
        assert_eq!(rel.key_events[0].event_type, "give");
    }

    #[test]
    fn test_record_social_event_accumulates_favorability() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();

        // 多次事件
        store.record_social_event(target_id, "王五", 5, "give", "送了馒头", 5).unwrap();
        store.record_social_event(target_id, "王五", 10, "trade", "公平交易", 3).unwrap();
        store.record_social_event(target_id, "王五", 15, "steal", "偷了银子", -15).unwrap();

        let rel = store.get_relationship(target_id).unwrap().unwrap();
        assert_eq!(rel.favorability, -7); // 5 + 3 - 15 = -7
        assert_eq!(rel.key_events.len(), 3);
    }

    #[test]
    fn test_record_social_event_favorability_clamped() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();

        // 超过上限
        store.record_social_event(target_id, "赵六", 1, "give", "大礼", 80).unwrap();
        store.record_social_event(target_id, "赵六", 2, "give", "再送礼", 50).unwrap();

        let rel = store.get_relationship(target_id).unwrap().unwrap();
        assert_eq!(rel.favorability, 100); // clamped at 100
    }

    #[test]
    fn test_record_social_event_max_events_cleanup() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("relationships.db");
        let agent_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();

        let store = RelationshipStore::open(agent_id, &db_path).unwrap();

        // 插入 25 个事件（超过 max_events=20）
        for i in 0..25 {
            store.record_social_event(target_id, "测试", i, "talk", "对话", 1).unwrap();
        }

        let rel = store.get_relationship(target_id).unwrap().unwrap();
        assert_eq!(rel.key_events.len(), 20);
        // 最老的事件被清理，保留最新的
        assert_eq!(rel.key_events[0].tick_id, 24); // 最新的排第一
    }
}
