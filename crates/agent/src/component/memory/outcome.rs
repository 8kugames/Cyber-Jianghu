// ============================================================================
// Outcome Memory — 行动结果记忆（Hermes 模式）
// ============================================================================
//
// 记录"做了X→效果Y"，供 LLM 未来决策参考。
// 场景指纹（context_hash）用于匹配相似场景下的历史经验。
// SQLite 持久化，轻量查询。
// ============================================================================

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use tracing::debug;

/// 行动结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutcomeResult {
    Success,
    Failed(String),
}

impl OutcomeResult {
    /// 规范化类型标签（用于 SQLite 存储，解耦 serde 序列化格式）
    fn type_tag(&self) -> &'static str {
        match self {
            OutcomeResult::Success => "success",
            OutcomeResult::Failed(_) => "failed",
        }
    }
}

/// 行动结果记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeRecord {
    /// action_type
    pub action_type: String,
    /// action_data（精简版，只保留关键字段）
    pub action_data: Option<serde_json::Value>,
    /// 执行结果
    pub result: OutcomeResult,
    /// 场景指纹（位置 + 附近物品类型 + NPC 数量）
    pub context_hash: String,
    /// 时间戳（tick_id）
    pub tick_id: i64,
}

/// 行动结果记忆
///
/// SQLite 持久化，记录每次行动的成功/失败。
/// 提供按 action_type 和 context_hash 的查询。
pub struct OutcomeMemory {
    conn: Mutex<Connection>,
    /// prompt 注入时每种 action 最多显示多少条
    prompt_limit: usize,
    /// 数据库最大记录数
    max_records: usize,
}

impl OutcomeMemory {
    /// 创建 OutcomeMemory（使用指定路径的 SQLite）
    pub fn new(db_path: &Path, prompt_limit: usize) -> Result<Self> {
        Self::with_max_records(db_path, prompt_limit, 1000)
    }

    /// 创建 OutcomeMemory（指定最大记录数）
    pub fn with_max_records(
        db_path: &Path,
        prompt_limit: usize,
        max_records: usize,
    ) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).context("创建 outcome memory 目录失败")?;
        }
        let conn = Connection::open(db_path).context("打开 outcome memory 数据库失败")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS outcome_records (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                action_type  TEXT NOT NULL,
                action_data  TEXT,
                result_type  TEXT NOT NULL,
                result       TEXT,
                context_hash TEXT NOT NULL,
                tick_id      INTEGER NOT NULL,
                created_at   INTEGER DEFAULT (strftime('%s', 'now'))
            );
            CREATE INDEX IF NOT EXISTS idx_outcome_action ON outcome_records(action_type);
            CREATE INDEX IF NOT EXISTS idx_outcome_context ON outcome_records(context_hash);
            ",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            prompt_limit,
            max_records,
        })
    }

    /// 记录行动结果
    pub fn record(&self, record: OutcomeRecord) {
        let result_type = record.result.type_tag();
        let result_detail = match &record.result {
            OutcomeResult::Failed(reason) => Some(reason.clone()),
            OutcomeResult::Success => None,
        };
        let action_data_str = record
            .action_data
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default());
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(e) => {
                debug!("outcome memory lock failed: {}", e);
                return;
            }
        };
        if let Err(e) = conn.execute(
            "INSERT INTO outcome_records (action_type, action_data, result_type, result, context_hash, tick_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![record.action_type, action_data_str, result_type, result_detail, record.context_hash, record.tick_id],
        ) {
            debug!("outcome memory record failed: {}", e);
        }
        drop(conn);

        // 自动清理：每 100 次写入触发一次
        if self.max_records > 0 {
            self.cleanup(self.max_records);
        }
    }

    /// 查询某 action_type 的近期记录
    pub fn query_recent(&self, action_type: &str, limit: usize) -> Vec<OutcomeRecord> {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut stmt = match conn.prepare(
            "SELECT action_type, action_data, result_type, result, context_hash, tick_id
             FROM outcome_records WHERE action_type = ?1 ORDER BY id DESC LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(params![action_type, limit], |row| {
            let action_type: String = row.get(0)?;
            let action_data_str: Option<String> = row.get(1)?;
            let result_type: String = row.get(2)?;
            let result_detail: Option<String> = row.get(3)?;
            let context_hash: String = row.get(4)?;
            let tick_id: i64 = row.get(5)?;
            let result = match result_type.as_str() {
                "success" => OutcomeResult::Success,
                _ => OutcomeResult::Failed(result_detail.unwrap_or_default()),
            };
            Ok(OutcomeRecord {
                action_type,
                action_data: action_data_str.and_then(|s| serde_json::from_str(&s).ok()),
                result,
                context_hash,
                tick_id,
            })
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// 获取某 action 的成功率
    pub fn success_rate(&self, action_type: &str) -> f64 {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return 0.0,
        };
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM outcome_records WHERE action_type = ?1",
                params![action_type],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if total == 0 {
            return 0.0;
        }
        let success: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM outcome_records WHERE action_type = ?1 AND result_type = 'success'",
                params![action_type],
                |row| row.get(0),
            )
            .unwrap_or(0);
        success as f64 / total as f64
    }

    /// 获取所有有记录的 action_type（动态查询，不硬编码）
    fn distinct_action_types(&self) -> Vec<String> {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut stmt = match conn
            .prepare("SELECT DISTINCT action_type FROM outcome_records ORDER BY action_type")
        {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map([], |row| {
            let at: String = row.get(0)?;
            Ok(at)
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// 生成 prompt 注入文本（经验教训段）
    pub fn to_prompt_context(&self) -> String {
        let action_types = self.distinct_action_types();
        let mut lines: Vec<String> = Vec::new();

        for at in &action_types {
            let records = self.query_recent(at, self.prompt_limit);
            if records.is_empty() {
                continue;
            }
            let success_count = records
                .iter()
                .filter(|r| matches!(r.result, OutcomeResult::Success))
                .count();
            let fail_count = records.len() - success_count;

            if success_count > 0 {
                lines.push(format!("- {} → 成功 [{}次]", at, success_count));
            }
            if fail_count > 0
                && let Some(OutcomeRecord {
                    result: OutcomeResult::Failed(reason),
                    ..
                }) = records
                    .iter()
                    .find(|r| matches!(r.result, OutcomeResult::Failed(_)))
            {
                let short_reason: String = reason.chars().take(30).collect();
                lines.push(format!(
                    "- {} → 失败（{}）[{}次]",
                    at, short_reason, fail_count
                ));
            }
        }

        if lines.is_empty() {
            return String::new();
        }

        format!("\n### 经验教训\n{}\n", lines.join("\n"))
    }

    /// 清理旧记录（保留最近 N 条）
    pub fn cleanup(&self, max_records: usize) {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return,
        };
        if let Err(e) = conn.execute(
            "DELETE FROM outcome_records WHERE id NOT IN (SELECT id FROM outcome_records ORDER BY id DESC LIMIT ?1)",
            params![max_records],
        ) {
            debug!("outcome memory cleanup failed: {}", e);
        }
    }
}

/// 生成场景指纹
///
/// 基于位置、附近物品类型、NPC 数量生成简单的标识。
/// 用于匹配相似场景下的历史经验。
pub fn compute_context_hash(world_state: &cyber_jianghu_protocol::WorldState) -> String {
    let location = &world_state.location.node_id;
    let item_types: Vec<&str> = world_state
        .nearby_items
        .iter()
        .map(|i| i.item_type.as_str())
        .collect();
    let entity_count = world_state.entities.len();
    format!("{}:{}:{}", location, item_types.join(","), entity_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_db() -> PathBuf {
        std::env::temp_dir().join(format!("outcome_test_{}.db", uuid::Uuid::new_v4()))
    }

    #[test]
    fn test_record_and_query() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).unwrap();

        mem.record(OutcomeRecord {
            action_type: "eat".into(),
            action_data: Some(serde_json::json!({"item_id": "mantou"})),
            result: OutcomeResult::Success,
            context_hash: "longmen_lobby:food,drink:2".into(),
            tick_id: 100,
        });

        mem.record(OutcomeRecord {
            action_type: "eat".into(),
            action_data: Some(serde_json::json!({"item_id": "invalid"})),
            result: OutcomeResult::Failed("物品不存在".into()),
            context_hash: "longmen_lobby:food,drink:2".into(),
            tick_id: 101,
        });

        let records = mem.query_recent("eat", 10);
        assert_eq!(records.len(), 2);
        assert!(matches!(records[0].result, OutcomeResult::Failed(_)));
        assert!(matches!(records[1].result, OutcomeResult::Success));

        let rate = mem.success_rate("eat");
        assert!((rate - 0.5).abs() < 0.01);

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn test_prompt_context() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).unwrap();

        mem.record(OutcomeRecord {
            action_type: "move".into(),
            action_data: Some(serde_json::json!({"target_location": "longmen_kitchen"})),
            result: OutcomeResult::Success,
            context_hash: "longmen_lobby::1".into(),
            tick_id: 100,
        });

        let ctx = mem.to_prompt_context();
        assert!(ctx.contains("经验教训"));
        assert!(ctx.contains("move → 成功"));

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn test_dynamic_action_types() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).unwrap();

        mem.record(OutcomeRecord {
            action_type: "attack".into(),
            action_data: None,
            result: OutcomeResult::Success,
            context_hash: "loc::0".into(),
            tick_id: 100,
        });

        let types = mem.distinct_action_types();
        assert!(types.contains(&"attack".to_string()));

        let ctx = mem.to_prompt_context();
        assert!(ctx.contains("attack → 成功"));

        let _ = std::fs::remove_file(&db);
    }
}
