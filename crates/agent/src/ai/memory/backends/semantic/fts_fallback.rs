// ============================================================================
// FTS 降级逻辑
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 当向量检索不可用时，使用 SQLite FTS5 全文检索作为降级方案
// ============================================================================

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;

/// FTS 降级处理器
///
/// 使用 SQLite FTS5 进行全文检索
pub struct FtsFallback {
    /// 数据库连接
    conn: Mutex<Connection>,
    /// Agent ID
    agent_id: Uuid,
}

impl FtsFallback {
    /// 创建新的 FTS 降级处理器
    pub fn new(agent_id: Uuid, db_path: &Path) -> Result<Self> {
        // 确保目录存在
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .context("Failed to create database directory")?;
        }

        let conn = Connection::open(db_path)
            .context("Failed to open FTS database")?;

        Self::init_schema(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
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

    /// 重建 FTS 索引
    pub fn rebuild_index(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        // 重建 FTS 索引
        conn.execute(
            "INSERT INTO memories_fts(memories_fts) VALUES('rebuild')",
            [],
        )
        .context("Failed to rebuild FTS index")?;

        tracing::info!("FTS index rebuilt");
        Ok(())
    }

    /// 全文检索
    ///
    /// 返回匹配记忆的 ID 列表
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<i64>> {
        let conn = self.conn.lock().unwrap();

        // 对查询进行转义和预处理
        let fts_query = self.prepare_query(query);

        let mut stmt = conn
            .prepare(
                "SELECT m.id
                 FROM client_memories m
                 JOIN memories_fts fts ON m.id = fts.rowid
                 WHERE memories_fts MATCH ?1
                   AND m.agent_id = ?2
                   AND m.is_archived = FALSE
                 ORDER BY bm25(memories_fts) ASC
                 LIMIT ?3",
            )
            .context("Failed to prepare FTS search query")?;

        let ids = stmt
            .query_map(params![fts_query, self.agent_id.to_string(), limit as i64], |row| {
                row.get(0)
            })
            .context("Failed to execute FTS search query")?;

        ids.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.into())
    }

    /// 模糊搜索（使用 LIKE 作为最后的降级方案）
    pub fn search_like(&self, pattern: &str, limit: usize) -> Result<Vec<i64>> {
        let conn = self.conn.lock().unwrap();

        let like_pattern = format!("%{}%", pattern);

        let mut stmt = conn
            .prepare(
                "SELECT id FROM client_memories
                 WHERE agent_id = ?1
                   AND content LIKE ?2
                   AND is_archived = FALSE
                 ORDER BY importance_score DESC
                 LIMIT ?3",
            )
            .context("Failed to prepare LIKE search query")?;

        let ids = stmt
            .query_map(params![self.agent_id.to_string(), like_pattern, limit as i64], |row| {
                row.get(0)
            })
            .context("Failed to execute LIKE search query")?;

        ids.collect::<Result<Vec<_>, _>>()
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

    /// 检查 FTS 是否可用
    pub fn is_available(&self) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute("SELECT 1 FROM memories_fts LIMIT 1", [])
            .is_ok()
    }

    /// 获取索引统计
    pub fn stats(&self) -> Result<FtsStats> {
        let conn = self.conn.lock().unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories_fts",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        Ok(FtsStats { document_count: count as usize })
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
        let db_path = temp_dir.path().join("test.db");

        // 创建基础表结构（测试需要）
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
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

        let fts = FtsFallback::new(agent_id, &db_path).unwrap();

        // 测试普通查询
        let query = fts.prepare_query("战斗");
        assert!(query.contains("战斗"));

        // 测试特殊字符转义
        let query_with_special = fts.prepare_query("test-value");
        assert!(query_with_special.contains("-"));
    }

    #[test]
    fn test_search_like() {
        let agent_id = Uuid::new_v4();
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
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

        // 插入测试数据
        conn.execute(
            "INSERT INTO client_memories (agent_id, content, importance_score) VALUES (?1, ?2, ?3)",
            params![agent_id.to_string(), "战斗胜利，获得了奖励", 0.8],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO client_memories (agent_id, content, importance_score) VALUES (?1, ?2, ?3)",
            params![agent_id.to_string(), "购买物品成功", 0.5],
        )
        .unwrap();

        let fts = FtsFallback::new(agent_id, &db_path).unwrap();

        let results = fts.search_like("战斗", 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_stats() {
        let agent_id = Uuid::new_v4();
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
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

        let fts = FtsFallback::new(agent_id, &db_path).unwrap();
        let stats = fts.stats().unwrap();

        // 空索引
        assert_eq!(stats.document_count, 0);
    }
}
