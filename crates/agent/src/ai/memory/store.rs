// ============================================================================
// SQLite 存储层
// ============================================================================
//
// 实现客户端记忆的持久化存储
// 支持情景记忆的长期保存和查询
// ============================================================================

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// 客户端记忆存储
pub struct MemoryStore {
    /// 数据库连接
    conn: Connection,
    /// 数据库路径
    db_path: PathBuf,
    /// Agent ID
    agent_id: Uuid,
}

impl MemoryStore {
    /// 初始化记忆存储
    ///
    /// 如果数据库不存在，将自动创建
    pub fn new(agent_id: Uuid, db_dir: &Path) -> Result<Self> {
        // 确保目录存在
        std::fs::create_dir_all(db_dir).context("Failed to create database directory")?;

        // 构建数据库路径
        let db_path = db_dir.join(format!("agent_{}.db", agent_id));

        // 打开数据库连接
        let conn = Connection::open(&db_path).context("Failed to open database")?;

        // 初始化数据库结构
        Self::init_schema(&conn)?;

        Ok(Self {
            conn,
            db_path,
            agent_id,
        })
    }

    /// 初始化数据库结构
    fn init_schema(conn: &Connection) -> Result<()> {
        // 创建主表
        conn.execute(
            "CREATE TABLE IF NOT EXISTS client_memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL,
                tick_id INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT,
                importance_score REAL DEFAULT 0.5,
                sentiment_score REAL DEFAULT 0.0,
                memory_type TEXT DEFAULT 'episodic',
                is_confirmed BOOLEAN DEFAULT TRUE,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )
        .context("Failed to create client_memories table")?;

        // 创建索引
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_memories_agent_id
             ON client_memories(agent_id)",
            [],
        )
        .ok();

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_memories_importance
             ON client_memories(importance_score DESC)",
            [],
        )
        .ok();

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_memories_created
             ON client_memories(created_at DESC)",
            [],
        )
        .ok();

        // 性能优化
        conn.execute("PRAGMA journal_mode = WAL", []).ok();
        conn.execute("PRAGMA synchronous = NORMAL", []).ok();
        conn.execute("PRAGMA cache_size = -64000", []).ok(); // 64MB cache

        Ok(())
    }

    /// 添加记忆
    pub fn add_memory(&self, memory: &ClientMemory) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO client_memories
             (agent_id, tick_id, event_type, content, metadata,
              importance_score, sentiment_score, memory_type, is_confirmed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    memory.agent_id.to_string(),
                    memory.tick_id,
                    &memory.event_type,
                    &memory.content,
                    memory.metadata.to_string(),
                    memory.importance_score,
                    memory.sentiment_score,
                    &memory.memory_type,
                    memory.is_confirmed,
                ],
            )
            .context("Failed to insert memory")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// 批量添加记忆
    pub fn add_memories_batch(&self, memories: &[ClientMemory]) -> Result<usize> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("Failed to begin transaction")?;

        for memory in memories {
            tx.execute(
                "INSERT INTO client_memories
                 (agent_id, tick_id, event_type, content, metadata,
                  importance_score, sentiment_score, memory_type, is_confirmed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    memory.agent_id.to_string(),
                    memory.tick_id,
                    &memory.event_type,
                    &memory.content,
                    memory.metadata.to_string(),
                    memory.importance_score,
                    memory.sentiment_score,
                    &memory.memory_type,
                    memory.is_confirmed,
                ],
            )
            .context("Failed to insert memory in batch")?;
        }

        tx.commit().context("Failed to commit transaction")?;
        Ok(memories.len())
    }

    /// 查询重要记忆（Top K）
    pub fn get_top_memories(&self, limit: usize) -> Result<Vec<ClientMemory>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM client_memories
             WHERE agent_id = ?1
             ORDER BY importance_score DESC, created_at DESC
             LIMIT ?2",
            )
            .context("Failed to prepare query")?;

        let memories = stmt
            .query_map(params![self.agent_id.to_string(), limit as i64], |row| {
                Ok(ClientMemory {
                    id: Some(row.get(0)?),
                    agent_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                    tick_id: row.get(2)?,
                    event_type: row.get(3)?,
                    content: row.get(4)?,
                    metadata: row
                        .get::<_, Option<String>>(5)?
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or(Value::Null),
                    importance_score: row.get(6)?,
                    sentiment_score: row.get(7)?,
                    memory_type: row.get(8)?,
                    is_confirmed: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })
            .context("Failed to execute query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 查询最近 N 条记忆
    pub fn get_recent_memories(&self, limit: usize) -> Result<Vec<ClientMemory>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM client_memories
             WHERE agent_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
            )
            .context("Failed to prepare query")?;

        let memories = stmt
            .query_map(params![self.agent_id.to_string(), limit as i64], |row| {
                Ok(ClientMemory {
                    id: Some(row.get(0)?),
                    agent_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                    tick_id: row.get(2)?,
                    event_type: row.get(3)?,
                    content: row.get(4)?,
                    metadata: row
                        .get::<_, Option<String>>(5)?
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or(Value::Null),
                    importance_score: row.get(6)?,
                    sentiment_score: row.get(7)?,
                    memory_type: row.get(8)?,
                    is_confirmed: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })
            .context("Failed to execute query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 按事件类型查询记忆
    pub fn get_memories_by_type(
        &self,
        event_type: &str,
        limit: usize,
    ) -> Result<Vec<ClientMemory>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM client_memories
             WHERE agent_id = ?1 AND event_type = ?2
             ORDER BY created_at DESC
             LIMIT ?3",
            )
            .context("Failed to prepare query")?;

        let memories = stmt
            .query_map(
                params![self.agent_id.to_string(), event_type, limit as i64],
                |row| {
                    Ok(ClientMemory {
                        id: Some(row.get(0)?),
                        agent_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                        tick_id: row.get(2)?,
                        event_type: row.get(3)?,
                        content: row.get(4)?,
                        metadata: row
                            .get::<_, Option<String>>(5)?
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or(Value::Null),
                        importance_score: row.get(6)?,
                        sentiment_score: row.get(7)?,
                        memory_type: row.get(8)?,
                        is_confirmed: row.get(9)?,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
                    })
                },
            )
            .context("Failed to execute query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 获取记忆总数
    pub fn count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM client_memories WHERE agent_id = ?1",
                params![self.agent_id.to_string()],
                |row| row.get(0),
            )
            .context("Failed to count memories")?;

        Ok(count as usize)
    }

    /// 清理旧记忆（保留最近 N 条）
    pub fn cleanup_old_memories(&self, keep_count: usize) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM client_memories
             WHERE agent_id = ?1 AND id NOT IN (
                 SELECT id FROM client_memories
                 WHERE agent_id = ?1
                 ORDER BY created_at DESC
                 LIMIT ?2
             )",
                params![self.agent_id.to_string(), keep_count as i64],
            )
            .context("Failed to cleanup old memories")
    }

    /// 清空所有记忆
    pub fn clear_all(&self) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM client_memories WHERE agent_id = ?1",
                params![self.agent_id.to_string()],
            )
            .context("Failed to clear memories")?;
        Ok(())
    }

    /// 获取数据库路径
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// 获取 Agent ID
    pub fn agent_id(&self) -> Uuid {
        self.agent_id
    }
}

/// 客户端记忆
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClientMemory {
    /// 记忆 ID（数据库自增）
    pub id: Option<i64>,
    /// Agent ID
    pub agent_id: Uuid,
    /// Tick 编号
    pub tick_id: i64,
    /// 事件类型
    pub event_type: String,
    /// 事件内容（自然语言）
    pub content: String,
    /// 元数据（JSON 格式）
    pub metadata: Value,
    /// 重要性评分（0.0-1.0）
    pub importance_score: f32,
    /// 情感评分（-1.0 负面 ~ 1.0 正面）
    pub sentiment_score: f32,
    /// 记忆类型（working, episodic, semantic）
    pub memory_type: String,
    /// 是否已确认（服务端确认的事件）
    pub is_confirmed: bool,
    /// 创建时间
    pub created_at: String,
    /// 更新时间
    pub updated_at: String,
}

impl ClientMemory {
    /// 创建新的记忆
    pub fn new(agent_id: Uuid, tick_id: i64, content: String) -> Self {
        Self {
            id: None,
            agent_id,
            tick_id,
            event_type: "unknown".to_string(),
            content,
            metadata: Value::Null,
            importance_score: 0.5,
            sentiment_score: 0.0,
            memory_type: "episodic".to_string(),
            is_confirmed: true,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    /// 设置事件类型
    pub fn with_type(mut self, event_type: String) -> Self {
        self.event_type = event_type;
        self
    }

    /// 设置重要性评分
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance_score = importance;
        self
    }

    /// 设置元数据
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// 设置记忆类型
    pub fn with_memory_type(mut self, memory_type: String) -> Self {
        self.memory_type = memory_type;
        self
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
    fn test_memory_store_init() {
        let temp_dir = TempDir::new().unwrap();
        let agent_id = Uuid::new_v4();

        let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

        assert_eq!(store.agent_id(), agent_id);
        assert!(store.db_path().exists());
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn test_add_and_retrieve_memory() {
        let temp_dir = TempDir::new().unwrap();
        let agent_id = Uuid::new_v4();
        let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

        let memory = ClientMemory::new(agent_id, 1, "测试记忆".to_string())
            .with_importance(0.8)
            .with_type("test".to_string());

        let id = store.add_memory(&memory).unwrap();
        assert!(id > 0);

        let memories = store.get_top_memories(10).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].content, "测试记忆");
        assert_eq!(memories[0].importance_score, 0.8);
    }

    #[test]
    fn test_batch_insert() {
        let temp_dir = TempDir::new().unwrap();
        let agent_id = Uuid::new_v4();
        let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

        let memories: Vec<ClientMemory> = (1..=10)
            .map(|i| ClientMemory::new(agent_id, i, format!("记忆 {}", i)))
            .collect();

        store.add_memories_batch(&memories).unwrap();
        assert_eq!(store.count().unwrap(), 10);
    }

    #[test]
    fn test_cleanup_old_memories() {
        let temp_dir = TempDir::new().unwrap();
        let agent_id = Uuid::new_v4();
        let store = MemoryStore::new(agent_id, temp_dir.path()).unwrap();

        // 添加 10 条记忆
        for i in 1..=10 {
            let memory = ClientMemory::new(agent_id, i, format!("记忆 {}", i));
            store.add_memory(&memory).unwrap();
        }

        assert_eq!(store.count().unwrap(), 10);

        // 清理，只保留最近 5 条
        let cleaned = store.cleanup_old_memories(5).unwrap();
        assert_eq!(cleaned, 5);
        assert_eq!(store.count().unwrap(), 5);
    }
}
