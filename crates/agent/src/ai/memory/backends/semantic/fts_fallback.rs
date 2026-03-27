// ============================================================================
// FTS 降级逻辑
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 当向量检索不可用时，使用 SQLite FTS5 全文检索作为降级方案
// ============================================================================

use crate::ai::memory::store::ClientMemory;
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

pub struct FtsFallback {
    fts_conn: Mutex<Connection>,
    episodic_conn: Mutex<Connection>,
    agent_id: Uuid,
}

impl FtsFallback {
    pub fn new(agent_id: Uuid, fts_db_path: &Path, episodic_db_path: &Path) -> Result<Self> {
        if let Some(parent) = fts_db_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create database directory")?;
        }

        let fts_conn = Connection::open(fts_db_path).context("Failed to open FTS database")?;
        Self::init_schema(&fts_conn)?;

        let episodic_conn =
            Connection::open(episodic_db_path).context("Failed to open episodic database")?;

        Ok(Self {
            fts_conn: Mutex::new(fts_conn),
            episodic_conn: Mutex::new(episodic_conn),
            agent_id,
        })
    }

    /// 初始化 FTS 虚拟表
    fn init_schema(conn: &Connection) -> Result<()> {
        // 创建 FTS5 虚拟表
        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content,
                content='client_memories',
                content_rowid='id',
                tokenize='unicode61'
            )",
            [],
        )
        .context("Failed to create FTS5 table")?;

        // 创建触发器以保持 FTS 索引同步
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON client_memories BEGIN
                INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
            END",
            [],
        )
        .ok();

        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON client_memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content);
            END",
            [],
        )
        .ok();

        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON client_memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('Delete', old.id, old.content);
                INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
            END",
            [],
        )
        .ok();

        // 性能优化
        conn.execute("PRAGMA journal_mode = WAL", []).ok();

        Ok(())
    }

    pub fn rebuild_index(&self) -> Result<()> {
        let fts_conn = self.fts_conn.lock().unwrap();

        fts_conn
            .execute(
                "INSERT INTO memories_fts(memories_fts) VALUES('rebuild')",
                [],
            )
            .context("Failed to rebuild FTS index")?;

        tracing::info!("FTS index rebuilt");
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<ClientMemory>> {
        let episodic_conn = self.episodic_conn.lock().unwrap();

        let fts_query = self.prepare_query(query);

        let mut stmt = episodic_conn
            .prepare(
                "SELECT id, agent_id, tick_id, event_type, content, metadata,
                    importance_score, sentiment_score, memory_type,
                    is_confirmed, created_at, updated_at
             FROM client_memories
             WHERE id IN (
                 SELECT fts.rowid FROM memories_fts fts WHERE memories_fts MATCH ?1
             )
             AND agent_id = ?2 AND is_archived = FALSE
             ORDER BY bm25(memories_fts) ASC
             LIMIT ?3",
            )
            .context("Failed to prepare FTS search query")?;

        let memories = stmt
            .query_map(
                params![fts_query, self.agent_id.to_string(), limit as i64],
                |row| {
                    Ok(ClientMemory {
                        id: row.get(0)?,
                        agent_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                        tick_id: row.get(2)?,
                        event_type: row.get(3)?,
                        content: row.get(4)?,
                        metadata: row
                            .get::<_, Option<String>>(5)?
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or_default(),
                        importance_score: row.get(6)?,
                        sentiment_score: row.get(7)?,
                        memory_type: row.get(8)?,
                        is_confirmed: row.get(9)?,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
                    })
                },
            )
            .context("Failed to execute FTS search query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    pub fn search_like(&self, pattern: &str, limit: usize) -> Result<Vec<ClientMemory>> {
        let episodic_conn = self.episodic_conn.lock().unwrap();

        let like_pattern = format!("%{}%", pattern);

        let mut stmt = episodic_conn
            .prepare(
                "SELECT id, agent_id, tick_id, event_type, content, metadata,
                    importance_score, sentiment_score, memory_type,
                    is_confirmed, created_at, updated_at
             FROM client_memories
             WHERE agent_id = ?1
               AND content LIKE ?2
               AND is_archived = FALSE
             ORDER BY importance_score DESC
             LIMIT ?3",
            )
            .context("Failed to prepare LIKE search query")?;

        let memories = stmt
            .query_map(
                params![self.agent_id.to_string(), like_pattern, limit as i64],
                |row| {
                    Ok(ClientMemory {
                        id: row.get(0)?,
                        agent_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                        tick_id: row.get(2)?,
                        event_type: row.get(3)?,
                        content: row.get(4)?,
                        metadata: row
                            .get::<_, Option<String>>(5)?
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or_default(),
                        importance_score: row.get(6)?,
                        sentiment_score: row.get(7)?,
                        memory_type: row.get(8)?,
                        is_confirmed: row.get(9)?,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
                    })
                },
            )
            .context("Failed to execute LIKE search query")?;

        memories
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 准备 FTS 查询
    ///
    /// - 处理特殊字符
    /// - 添加通配符
    fn prepare_query(&self, query: &str) -> String {
        // FTS5 特殊字符需要转义
        let special_chars = ['"', '\'', '-', '*', '^', ':', '(', ')', '{', '}'];

        let mut escaped = String::new();
        for c in query.chars() {
            if special_chars.contains(&c) {
                escaped.push('"');
                escaped.push(c);
                escaped.push('"');
            } else {
                escaped.push(c);
            }
        }

        // 如果查询不是以通配符结尾，添加通配符
        if !escaped.ends_with('*') {
            format!("{}*", escaped)
        } else {
            escaped
        }
    }

    pub fn is_available(&self) -> bool {
        let fts_conn = self.fts_conn.lock().unwrap();
        fts_conn
            .execute("SELECT 1 FROM memories_fts LIMIT 1", [])
            .is_ok()
    }

    pub fn get_memories_by_ids(&self, ids: &[i64]) -> Result<Vec<ClientMemory>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let episodic_conn = self.episodic_conn.lock().unwrap();
        let agent_id_str = self.agent_id.to_string();

        let mut memories = Vec::new();
        for &id in ids {
            let mut stmt = episodic_conn.prepare(
                "SELECT id, agent_id, tick_id, event_type, content, metadata,
                        importance_score, sentiment_score, memory_type,
                        is_confirmed, created_at, updated_at
                 FROM client_memories WHERE id = ?1 AND agent_id = ?2",
            )?;

            let result = stmt.query_row(params![id, agent_id_str], |row| {
                Ok(ClientMemory {
                    id: row.get(0)?,
                    agent_id: Uuid::parse_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                    tick_id: row.get(2)?,
                    event_type: row.get(3)?,
                    content: row.get(4)?,
                    metadata: row
                        .get::<_, Option<String>>(5)?
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default(),
                    importance_score: row.get(6)?,
                    sentiment_score: row.get(7)?,
                    memory_type: row.get(8)?,
                    is_confirmed: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            });

            if let Ok(memory) = result {
                memories.push(memory);
            }
        }

        Ok(memories)
    }

    pub fn stats(&self) -> Result<FtsStats> {
        let fts_conn = self.fts_conn.lock().unwrap();

        let count: i64 = fts_conn
            .query_row("SELECT COUNT(*) FROM memories_fts", [], |row| row.get(0))
            .unwrap_or(0);

        Ok(FtsStats {
            document_count: count as usize,
        })
    }
}

/// FTS 统计信息
#[derive(Debug, Clone)]
pub struct FtsStats {
    /// 文档数量
    pub document_count: usize,
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_prepare_query() {
        let agent_id = Uuid::new_v4();
        let temp_dir = TempDir::new().unwrap();
        let episodic_path = temp_dir.path().join("episodic.db");
        let fts_path = temp_dir.path().join("fts.db");

        let episodic_conn = Connection::open(&episodic_path).unwrap();
        episodic_conn
            .execute(
                "CREATE TABLE IF NOT EXISTS client_memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT NOT NULL,
                content TEXT NOT NULL,
                importance_score REAL DEFAULT 0.5,
                is_archived BOOLEAN DEFAULT FALSE
            )",
                [],
            )
            .unwrap();

        let fts = FtsFallback::new(agent_id, &fts_path, &episodic_path).unwrap();

        let query = fts.prepare_query("战斗");
        assert!(query.contains("战斗"));

        let query_with_special = fts.prepare_query("test-value");
        assert!(query_with_special.contains("-"));
    }

    #[test]
    fn test_search_like() {
        let agent_id = Uuid::new_v4();
        let temp_dir = TempDir::new().unwrap();
        let episodic_path = temp_dir.path().join("episodic.db");
        let fts_path = temp_dir.path().join("fts.db");

        let episodic_conn = Connection::open(&episodic_path).unwrap();
        episodic_conn
            .execute(
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
                updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                is_archived BOOLEAN DEFAULT FALSE
            )",
                [],
            )
            .unwrap();

        episodic_conn.execute(
            "INSERT INTO client_memories (agent_id, tick_id, event_type, content, importance_score) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![agent_id.to_string(), 1, "combat", "战斗胜利，获得了奖励", 0.8],
        ).unwrap();

        episodic_conn.execute(
            "INSERT INTO client_memories (agent_id, tick_id, event_type, content, importance_score) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![agent_id.to_string(), 2, "trade", "购买物品成功", 0.5],
        ).unwrap();

        let fts = FtsFallback::new(agent_id, &fts_path, &episodic_path).unwrap();

        let results = fts.search_like("战斗", 10).unwrap();
        assert_eq!(results.len(), 1);
    }
}
