// ============================================================================
// SQLite 存储层
// ============================================================================
//
// 实现客户端记忆的持久化存储
// 支持情景记忆的长期保存和查询
// ============================================================================

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
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

        // 渐进式迁移：添加遗忘机制所需的新列
        Self::migrate_forgetting_columns(&conn)?;

        // 渐进式迁移：添加 embedding 向量列
        Self::migrate_embedding_column(&conn)?;
        Self::migrate_encoding_columns(&conn)?;

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

    /// 渐进式迁移：检查并添加遗忘机制所需的新列（幂等操作）
    fn migrate_forgetting_columns(conn: &Connection) -> Result<()> {
        let columns = vec![
            ("strength", "REAL DEFAULT 0.5"),
            ("last_accessed_at", "TIMESTAMP"),
            ("access_count", "INTEGER DEFAULT 0"),
            ("is_archived", "BOOLEAN DEFAULT FALSE"),
        ];

        for (column, col_type) in columns {
            let exists: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('client_memories') WHERE name='{}'",
                        column
                    ),
                    [],
                    |row| row.get(0),
                )
                .context("Failed to check column existence")?;

            if exists == 0 {
                conn.execute(
                    &format!(
                        "ALTER TABLE client_memories ADD COLUMN {} {}",
                        column, col_type
                    ),
                    [],
                )
                .with_context(|| format!("Failed to add column: {}", column))?;
            }
        }

        Ok(())
    }

    /// 渐进式迁移：添加 embedding 向量列（幂等操作）
    fn migrate_embedding_column(conn: &Connection) -> Result<()> {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('client_memories') WHERE name='embedding'",
                [],
                |row| row.get(0),
            )
            .context("Failed to check embedding column existence")?;

        if exists == 0 {
            conn.execute("ALTER TABLE client_memories ADD COLUMN embedding BLOB", [])
                .context("Failed to add embedding column")?;
        }

        Ok(())
    }

    /// 渐进式迁移：添加情绪编码列（幂等操作）
    fn migrate_encoding_columns(conn: &Connection) -> Result<()> {
        let columns = vec![
            ("encoding_valence", "REAL"),
            ("encoding_arousal", "REAL"),
            ("encoding_emotion", "TEXT"),
        ];
        for (column, col_type) in columns {
            let exists: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('client_memories') WHERE name='{}'",
                        column
                    ),
                    [],
                    |row| row.get(0),
                )
                .with_context(|| format!("Failed to check column existence: {}", column))?;
            if exists == 0 {
                conn.execute(
                    &format!(
                        "ALTER TABLE client_memories ADD COLUMN {} {}",
                        column, col_type
                    ),
                    [],
                )
                .with_context(|| format!("Failed to add column: {}", column))?;
            }
        }
        Ok(())
    }

    /// 更新记忆的 embedding 向量
    pub fn update_embedding(&self, memory_id: i64, embedding: &[u8]) -> Result<()> {
        self.conn
            .execute(
                "UPDATE client_memories SET embedding = ?1 WHERE id = ?2",
                params![embedding, memory_id],
            )
            .context("Failed to update embedding")?;
        Ok(())
    }

    /// 获取记忆的 embedding 向量
    pub fn get_embedding(&self, memory_id: i64) -> Result<Option<Vec<u8>>> {
        let embedding = self
            .conn
            .query_row(
                "SELECT embedding FROM client_memories WHERE id = ?1 AND embedding IS NOT NULL",
                params![memory_id],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()
            .context("Failed to get embedding")?;
        Ok(embedding.unwrap_or(None))
    }

    /// 添加记忆
    pub fn add_memory(&self, memory: &ClientMemory) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO client_memories
             (agent_id, tick_id, event_type, content, metadata,
              importance_score, sentiment_score, memory_type, is_confirmed,
              created_at, updated_at,
              strength, last_accessed_at, access_count, is_archived,
              encoding_valence, encoding_arousal, encoding_emotion)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
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
                    &memory.created_at,
                    &memory.updated_at,
                    memory.strength,
                    memory.last_accessed_at.as_deref(),
                    memory.access_count,
                    memory.is_archived,
                    memory.encoding_valence,
                    memory.encoding_arousal,
                    memory.encoding_emotion,
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
                  importance_score, sentiment_score, memory_type, is_confirmed,
                  created_at, updated_at,
                  strength, last_accessed_at, access_count, is_archived,
                  encoding_valence, encoding_arousal, encoding_emotion)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
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
                    &memory.created_at,
                    &memory.updated_at,
                    memory.strength,
                    memory.last_accessed_at.as_deref(),
                    memory.access_count,
                    memory.is_archived,
                    memory.encoding_valence,
                    memory.encoding_arousal,
                    memory.encoding_emotion,
                ],
            )
            .context("Failed to insert memory in batch")?;
        }

        tx.commit().context("Failed to commit transaction")?;
        Ok(memories.len())
    }

    /// 从 SELECT * 行反序列化为 ClientMemory
    ///
    /// 列顺序：id(0), agent_id(1), tick_id(2), event_type(3), content(4),
    /// metadata(5), importance_score(6), sentiment_score(7), memory_type(8),
    /// is_confirmed(9), created_at(10), updated_at(11),
    /// strength(12), last_accessed_at(13), access_count(14), is_archived(15),
    /// encoding_valence(16), encoding_arousal(17), encoding_emotion(18)
    fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClientMemory> {
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
            strength: row.get(12)?,
            last_accessed_at: row.get(13)?,
            access_count: row.get(14)?,
            is_archived: row.get(15)?,
            encoding_valence: row.get(16)?,
            encoding_arousal: row.get(17)?,
            encoding_emotion: row.get(18)?,
        })
    }

    /// 查询重要记忆（Top K，排除已归档）
    pub fn get_top_memories(&self, limit: usize) -> Result<Vec<ClientMemory>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM client_memories
             WHERE agent_id = ?1 AND is_archived = FALSE
             ORDER BY importance_score DESC, created_at DESC
             LIMIT ?2",
            )
            .context("Failed to prepare query")?;

        let memories = stmt
            .query_map(
                params![self.agent_id.to_string(), limit as i64],
                Self::row_to_memory,
            )
            .context("Failed to execute query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 查询最近 N 条记忆（排除已归档）
    pub fn get_recent_memories(&self, limit: usize) -> Result<Vec<ClientMemory>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM client_memories
             WHERE agent_id = ?1 AND is_archived = FALSE
             ORDER BY created_at DESC
             LIMIT ?2",
            )
            .context("Failed to prepare query")?;

        let memories = stmt
            .query_map(
                params![self.agent_id.to_string(), limit as i64],
                Self::row_to_memory,
            )
            .context("Failed to execute query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 按事件类型查询记忆（排除已归档）
    pub fn get_memories_by_type(
        &self,
        event_type: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ClientMemory>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM client_memories
             WHERE agent_id = ?1 AND event_type = ?2 AND is_archived = FALSE
             ORDER BY created_at DESC
             LIMIT ?3 OFFSET ?4",
            )
            .context("Failed to prepare query")?;

        let memories = stmt
            .query_map(
                params![
                    self.agent_id.to_string(),
                    event_type,
                    limit as i64,
                    offset as i64
                ],
                Self::row_to_memory,
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

    /// 更新记忆强度（检索时调用，每次访问 +0.1，上限 1.0）
    pub fn update_strength(&self, id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE client_memories
                 SET strength = CASE WHEN strength + 0.1 > 1.0 THEN 1.0 ELSE strength + 0.1 END,
                     last_accessed_at = CURRENT_TIMESTAMP,
                     access_count = access_count + 1
                 WHERE id = ?1",
                params![id],
            )
            .context("Failed to update memory strength")?;

        Ok(())
    }

    /// 衰减所有未归档记忆的强度（遗忘机制调用）
    pub fn decay_strength(&self, decay_rate: f32) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE client_memories
                 SET strength = strength * ?1
                 WHERE agent_id = ?2 AND is_archived = FALSE",
                params![decay_rate, self.agent_id.to_string()],
            )
            .context("Failed to decay memory strength")?;

        let changes: i64 = self
            .conn
            .query_row("SELECT changes()", [], |row| row.get(0))
            .unwrap_or(0);

        Ok(changes as usize)
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

    /// 归档低强度记忆（strength < threshold）
    pub fn archive_weak_memories(&self, threshold: f32) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE client_memories
                 SET is_archived = TRUE
                 WHERE agent_id = ?1
                   AND strength < ?2
                   AND is_archived = FALSE",
                params![self.agent_id.to_string(), threshold],
            )
            .context("Failed to archive weak memories")?;

        let changes: i64 = self
            .conn
            .query_row("SELECT changes()", [], |row| row.get(0))
            .unwrap_or(0);

        Ok(changes as usize)
    }

    /// 效价一致性检索偏置查询
    pub fn get_top_memories_with_valence_bias(
        &self,
        limit: usize,
        current_valence: f32,
        valence_bias_weight: f32,
        valence_range: f32,
        null_encoding_bonus: f32,
    ) -> Result<Vec<ClientMemory>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM client_memories
                 WHERE agent_id = ?1 AND is_archived = FALSE
                 ORDER BY (importance_score +
                     CASE WHEN encoding_valence IS NULL THEN ?2
                     ELSE ?3 * MAX(0.0, 1.0 - ABS(encoding_valence - ?4) / ?5) END
                 ) DESC
                 LIMIT ?6",
            )
            .context("Failed to prepare valence-biased query")?;

        let memories = stmt
            .query_map(
                params![
                    self.agent_id.to_string(),
                    null_encoding_bonus,
                    valence_bias_weight,
                    current_valence,
                    valence_range,
                    limit as i64,
                ],
                Self::row_to_memory,
            )
            .context("Failed to execute valence-biased query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 获取已归档记忆
    pub fn get_archived_memories(&self, limit: usize) -> Result<Vec<ClientMemory>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM client_memories
                 WHERE agent_id = ?1 AND is_archived = TRUE
                 ORDER BY created_at DESC
                 LIMIT ?2",
            )
            .context("Failed to prepare archived query")?;

        let memories = stmt
            .query_map(
                params![self.agent_id.to_string(), limit as i64],
                Self::row_to_memory,
            )
            .context("Failed to execute archived query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 获取所有未归档记忆（遗忘机制使用）
    pub fn get_all_unarchived(&self) -> Result<Vec<ClientMemory>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT * FROM client_memories
                 WHERE agent_id = ?1 AND is_archived = FALSE
                 ORDER BY importance_score DESC",
            )
            .context("Failed to prepare unarchived query")?;

        let memories = stmt
            .query_map(params![self.agent_id.to_string()], Self::row_to_memory)
            .context("Failed to execute unarchived query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 根据 ID 获取单条记忆（检索后自动增强）
    pub fn get_by_id(&self, id: i64) -> Result<Option<ClientMemory>> {
        let result = self
            .conn
            .query_row(
                "SELECT * FROM client_memories WHERE id = ?1",
                params![id],
                Self::row_to_memory,
            )
            .optional()?;

        if result.is_some() {
            self.update_strength(id)?;
        }

        Ok(result)
    }

    /// 获取数据库路径
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// 获取 Agent ID
    pub fn agent_id(&self) -> Uuid {
        self.agent_id
    }

    /// 获取已归档记忆数量（直接 COUNT，不加载全量数据）
    pub fn count_archived(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM client_memories WHERE agent_id = ?1 AND is_archived = TRUE",
                params![self.agent_id.to_string()],
                |row| row.get(0),
            )
            .context("Failed to count archived memories")?;

        Ok(count as usize)
    }

    /// 批量归档指定 ID 的记忆
    pub fn archive_by_ids(&self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }

        let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
        let query = format!(
            "UPDATE client_memories SET is_archived = TRUE WHERE agent_id = ?1 AND id IN ({})",
            placeholders.join(",")
        );

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(self.agent_id.to_string())];
        for &id in ids {
            params_vec.push(Box::new(id));
        }
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        self.conn
            .execute(&query, params_refs.as_slice())
            .context("Failed to archive memories by ids")?;

        let changes: i64 = self
            .conn
            .query_row("SELECT changes()", [], |row| row.get(0))
            .unwrap_or(0);

        Ok(changes as usize)
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
    /// 记忆强度（0.0-1.0，用于遗忘计算）
    pub strength: f32,
    /// 最后访问时间（RFC3339）
    pub last_accessed_at: Option<String>,
    /// 访问次数
    pub access_count: i32,
    /// 是否已归档
    pub is_archived: bool,
    /// 编码时的效价
    pub encoding_valence: Option<f32>,
    /// 编码时的唤醒度
    pub encoding_arousal: Option<f32>,
    /// 编码时的情绪标签
    pub encoding_emotion: Option<String>,
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
            strength: 0.5,
            last_accessed_at: None,
            access_count: 0,
            is_archived: false,
            encoding_valence: None,
            encoding_arousal: None,
            encoding_emotion: None,
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
