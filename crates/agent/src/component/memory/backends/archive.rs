// ============================================================================
// 归档记忆后端
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 存储已遗忘的记忆，支持"努力回忆"功能
// ============================================================================

use crate::component::memory::backend::MemoryBackend;
use crate::component::memory::types::MemoryEntry;
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

/// 归档记忆后端
///
/// 存储已从 EpisodicMemory 遗忘的记忆
pub struct ArchiveMemoryBackend {
    /// 数据库连接
    conn: Mutex<Connection>,
    /// 数据库路径
    db_path: PathBuf,
    /// Agent ID
    agent_id: Uuid,
}

impl ArchiveMemoryBackend {
    /// 创建新的归档后端
    pub fn new(agent_id: Uuid, db_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(db_dir).context("Failed to create database directory")?;

        let db_path = db_dir.join(format!("agent_{}_archive.db", agent_id));
        let conn = Connection::open(&db_path).context("Failed to open archive database")?;

        Self::init_schema(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
            db_path,
            agent_id,
        })
    }

    /// 初始化数据库结构
    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS archived_memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                original_id INTEGER NOT NULL,
                agent_id TEXT NOT NULL,
                tick_id INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT,
                importance_score REAL,
                archived_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                original_created_at TIMESTAMP
            )",
            [],
        )
        .context("Failed to create archived_memories table")?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_archived_agent ON archived_memories(agent_id)",
            [],
        )
        .ok();

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_archived_tick ON archived_memories(tick_id)",
            [],
        )
        .ok();

        conn.execute("PRAGMA journal_mode = WAL", []).ok();

        Ok(())
    }

    /// 归档记忆（从 EpisodicMemory 移动过来）
    pub fn archive(&self, memory: &MemoryEntry, original_id: i64) -> Result<i64> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            "INSERT INTO archived_memories
             (original_id, agent_id, tick_id, event_type, content, metadata, importance_score, original_created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                original_id,
                memory.agent_id.to_string(),
                memory.tick_id,
                memory.event_type,
                memory.content,
                memory.metadata.to_string(),
                memory.importance_score,
                memory.created_at.to_rfc3339(),
            ],
        )
        .context("Failed to archive memory")?;

        Ok(conn.last_insert_rowid())
    }

    /// 搜索归档记忆（FTS 风格，使用 LIKE）
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT tick_id, event_type, content, metadata, importance_score, original_created_at
                 FROM archived_memories
                 WHERE agent_id = ?1 AND content LIKE ?2
                 ORDER BY importance_score DESC
                 LIMIT ?3",
            )
            .context("Failed to prepare search query")?;

        let pattern = format!("%{}%", query);
        let memories = stmt
            .query_map(
                params![self.agent_id.to_string(), pattern, limit as i64],
                |row| {
                    Ok(MemoryEntry::new(self.agent_id, row.get(0)?, row.get(2)?)
                        .with_event_type(row.get(1)?)
                        .with_importance(row.get(4)?)
                        .with_metadata(
                            row.get::<_, Option<String>>(3)?
                                .and_then(|s| serde_json::from_str(&s).ok())
                                .unwrap_or(serde_json::Value::Null),
                        ))
                },
            )
            .context("Failed to execute search query")?;

        let mut result = Vec::new();
        for memory in memories {
            result.push(memory?);
        }
        Ok(result)
    }

    /// 获取数据库路径
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}

#[async_trait::async_trait]
impl MemoryBackend for ArchiveMemoryBackend {
    fn name(&self) -> &'static str {
        "ArchiveMemory"
    }

    async fn add(&mut self, memory: MemoryEntry) -> Result<()> {
        // 归档记忆时，original_id 设为 0（表示没有原始 ID）
        self.archive(&memory, 0)?;
        Ok(())
    }

    async fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM archived_memories WHERE agent_id = ?1",
                params![self.agent_id.to_string()],
                |row| row.get(0),
            )
            .context("Failed to count archived memories")?;

        Ok(count as usize)
    }

    async fn clear(&mut self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM archived_memories WHERE agent_id = ?1",
            params![self.agent_id.to_string()],
        )
        .context("Failed to clear archived memories")?;

        Ok(())
    }
}

// 注意：ArchiveMemoryBackend 不实现 Default trait
// 因为它需要 agent_id 和 db_dir 参数才能初始化

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_backend() -> (ArchiveMemoryBackend, TempDir, Uuid) {
        let temp_dir = TempDir::new().unwrap();
        let agent_id = Uuid::new_v4();
        let backend = ArchiveMemoryBackend::new(agent_id, temp_dir.path()).unwrap();
        (backend, temp_dir, agent_id)
    }

    fn create_test_entry(agent_id: Uuid, content: &str, importance: f32) -> MemoryEntry {
        MemoryEntry::new(agent_id, 1, content.to_string()).with_importance(importance)
    }

    #[tokio::test]
    async fn test_add_and_count() {
        let (mut backend, _temp, agent_id) = create_test_backend();

        assert_eq!(backend.count().await.unwrap(), 0);

        backend
            .add(create_test_entry(agent_id, "测试记忆", 0.5))
            .await
            .unwrap();
        assert_eq!(backend.count().await.unwrap(), 1);

        backend
            .add(create_test_entry(agent_id, "另一个记忆", 0.6))
            .await
            .unwrap();
        assert_eq!(backend.count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_search() {
        let (mut backend, _temp, agent_id) = create_test_backend();

        backend
            .add(create_test_entry(agent_id, "战斗胜利", 0.8))
            .await
            .unwrap();
        backend
            .add(create_test_entry(agent_id, "购买物品", 0.5))
            .await
            .unwrap();
        backend
            .add(create_test_entry(agent_id, "战斗失败", 0.7))
            .await
            .unwrap();

        let results = backend.search("战斗", 10).unwrap();
        assert_eq!(results.len(), 2);

        // 重要性排序
        assert!(results[0].importance_score >= results[1].importance_score);
    }

    #[tokio::test]
    async fn test_clear() {
        let (mut backend, _temp, agent_id) = create_test_backend();

        backend
            .add(create_test_entry(agent_id, "记忆1", 0.5))
            .await
            .unwrap();
        backend
            .add(create_test_entry(agent_id, "记忆2", 0.5))
            .await
            .unwrap();
        assert_eq!(backend.count().await.unwrap(), 2);

        backend.clear().await.unwrap();
        assert_eq!(backend.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_archive_with_original_id() {
        let (backend, _temp, agent_id) = create_test_backend();
        let entry = create_test_entry(agent_id, "原始记忆", 0.7);

        let archived_id = backend.archive(&entry, 42).unwrap();
        assert!(archived_id > 0);
        assert_eq!(backend.count().await.unwrap(), 1);
    }
}
