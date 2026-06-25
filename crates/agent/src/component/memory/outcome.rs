// ============================================================================
// Outcome Memory — 行动结果记忆（Hermes 模式）
// ============================================================================
//
// 记录"做了X→效果Y"，供 LLM 决策参考。
// 场景指纹（context_hash）用于匹配相似场景下的历史经验。
// SQLite 持久化，轻量查询。
// ============================================================================

use anyhow::{Context, Result};
use rusqlite::{Connection, Row, params};
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
    /// 交互目标 Agent ID（社交类动作从 action_data 提取）
    #[serde(default)]
    pub target_agent_id: Option<String>,
    /// 场景指纹（位置 + 附近物品类型 + NPC 数量）
    pub context_hash: String,
    /// 时间戳（tick_id）
    pub tick_id: i64,
}

/// 从 action_data 中提取 target_agent_id
///
/// 提取优先级：target_agent_id > target_id > target_uuid
pub fn extract_target_agent_id(action_data: &Option<serde_json::Value>) -> Option<String> {
    let data = action_data.as_ref()?;
    for key in ["target_agent_id", "target_id", "target_uuid"] {
        if let Some(id) = data.get(key).and_then(|v| v.as_str())
            && !id.is_empty()
        {
            return Some(id.to_string());
        }
    }
    None
}

/// SQLite row → OutcomeRecord 映射（query_recent / query_by_target 共用）
fn row_to_outcome(row: &Row) -> rusqlite::Result<OutcomeRecord> {
    let action_type: String = row.get(0)?;
    let action_data_str: Option<String> = row.get(1)?;
    let result_type: String = row.get(2)?;
    let result_detail: Option<String> = row.get(3)?;
    let target_agent_id: Option<String> = row.get(4)?;
    let context_hash: String = row.get(5)?;
    let tick_id: i64 = row.get(6)?;
    let result = match result_type.as_str() {
        "success" => OutcomeResult::Success,
        _ => OutcomeResult::Failed(result_detail.unwrap_or_default()),
    };
    Ok(OutcomeRecord {
        action_type,
        action_data: action_data_str.and_then(|s| serde_json::from_str(&s).ok()),
        result,
        target_agent_id,
        context_hash,
        tick_id,
    })
}

/// 行动结果记忆
///
/// SQLite 持久化，记录每次行动的成功/失败。
/// 提供按 action_type、context_hash 和 target_agent_id 的查询。
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
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                action_type      TEXT NOT NULL,
                action_data      TEXT,
                result_type      TEXT NOT NULL,
                result           TEXT,
                target_agent_id  TEXT,
                context_hash     TEXT NOT NULL,
                tick_id          INTEGER NOT NULL,
                created_at       INTEGER DEFAULT (strftime('%s', 'now'))
            );
            CREATE INDEX IF NOT EXISTS idx_outcome_action ON outcome_records(action_type);
            CREATE INDEX IF NOT EXISTS idx_outcome_context ON outcome_records(context_hash);
            ",
        )?;
        // 老库迁移：先加列，再加索引（顺序关键：旧库无 target_agent_id 列）
        Self::migrate_legacy_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            prompt_limit,
            max_records,
        })
    }

    /// 老库 schema 迁移：仅在缺列时执行 ALTER，CREATE INDEX 错误必须显式传播。
    ///
    /// 之前 `let _ = conn.execute_batch(...)` 会吞掉只读 DB / 锁冲突 / schema 腐化
    /// 等真实错误，让初始化伪装成功。严格化后：缺列才 ALTER；任何非"已存在"
    /// 错误立刻冒泡，由 `with_max_records` 返回 Err。
    fn migrate_legacy_schema(conn: &Connection) -> Result<()> {
        if !Self::outcome_table_has_column(conn, "target_agent_id")? {
            conn.execute_batch("ALTER TABLE outcome_records ADD COLUMN target_agent_id TEXT")
                .context("迁移 outcome_records.target_agent_id 失败")?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_outcome_target ON outcome_records(target_agent_id)",
        )
        .context("创建 idx_outcome_target 索引失败")?;
        Ok(())
    }

    /// 检查 outcome_records 是否已存在指定列
    fn outcome_table_has_column(conn: &Connection, column: &str) -> Result<bool> {
        let mut stmt = conn
            .prepare("PRAGMA table_info(outcome_records)")
            .context("查询 outcome_records schema 失败")?;
        let mut rows = stmt.query([]).context("读取 outcome_records schema 行失败")?;
        while let Some(row) = rows.next().context("遍历 outcome_records schema 行失败")? {
            let name: String = row.get(1).context("读取 column name 失败")?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 记录行动结果
    ///
    /// P1-3 修复：返回 `Result<()>` 让 caller 显式处理 DB 错误。
    /// 之前用 `if let Err(e) = ... { debug!(...) }` 静默吞错 + `debug!` 级
    /// 默认不输出 → 运维看不到任何失败痕迹。
    pub fn record(&self, record: OutcomeRecord) -> anyhow::Result<()> {
        let result_type = record.result.type_tag();
        let result_detail = match &record.result {
            OutcomeResult::Failed(reason) => Some(reason.clone()),
            OutcomeResult::Success => None,
        };
        let action_data_str = record
            .action_data
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default());
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("outcome memory lock poisoned: {e}"))?;
        conn.execute(
            "INSERT INTO outcome_records (action_type, action_data, result_type, result, target_agent_id, context_hash, tick_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![record.action_type, action_data_str, result_type, result_detail, record.target_agent_id, record.context_hash, record.tick_id],
        )
        .context("outcome memory record insert 失败")?;
        drop(conn);

        // 自动清理：每 100 次写入触发一次
        if self.max_records > 0 {
            self.cleanup(self.max_records);
        }
        Ok(())
    }

    /// 查询某 action_type 的近期记录
    ///
    /// P1-3 修复：返回 `Result<Vec<_>>` 让 caller 区分"无记录"与"DB 错"。
    /// 之前静默返回空 Vec 会让下游把"DB 错"误判为"该 action_type 无历史"。
    pub fn query_recent(&self, action_type: &str, limit: usize) -> anyhow::Result<Vec<OutcomeRecord>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("outcome memory lock poisoned: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT action_type, action_data, result_type, result, target_agent_id, context_hash, tick_id
                 FROM outcome_records WHERE action_type = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .context("prepare query_recent 失败")?;
        let rows = stmt
            .query_map(params![action_type, limit], row_to_outcome)
            .context("execute query_recent 失败")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect query_recent rows 失败")
    }

    /// 查询与特定 Agent 的交互历史
    ///
    /// P1-3 修复：返回 `Result<Vec<_>>`。
    pub fn query_by_target(
        &self,
        target_agent_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<OutcomeRecord>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("outcome memory lock poisoned: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT action_type, action_data, result_type, result, target_agent_id, context_hash, tick_id
                 FROM outcome_records WHERE target_agent_id = ?1 ORDER BY id DESC LIMIT ?2",
            )
            .context("prepare query_by_target 失败")?;
        let rows = stmt
            .query_map(params![target_agent_id, limit], row_to_outcome)
            .context("execute query_by_target 失败")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect query_by_target rows 失败")
    }

    /// 获取某 action 的成功率
    ///
    /// P1-3 修复：返回 `Result<f64>`。
    /// 之前静默返回 0.0 会让 caller 把"DB 错"误判为"该 action 100% 失败"。
    pub fn success_rate(&self, action_type: &str) -> anyhow::Result<f64> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("outcome memory lock poisoned: {e}"))?;
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM outcome_records WHERE action_type = ?1",
                params![action_type],
                |row| row.get(0),
            )
            .context("count total records 失败")?;
        if total == 0 {
            return Ok(0.0);
        }
        let success: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM outcome_records WHERE action_type = ?1 AND result_type = 'success'",
                params![action_type],
                |row| row.get(0),
            )
            .context("count success records 失败")?;
        Ok(success as f64 / total as f64)
    }

    /// 获取所有有记录的 action_type（动态查询，不硬编码）
    fn distinct_action_types(&self) -> anyhow::Result<Vec<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("outcome memory lock poisoned: {e}"))?;
        let mut stmt = conn
            .prepare("SELECT DISTINCT action_type FROM outcome_records ORDER BY action_type")
            .context("prepare distinct_action_types 失败")?;
        let rows = stmt
            .query_map([], |row| {
                let at: String = row.get(0)?;
                Ok(at)
            })
            .context("execute distinct_action_types 失败")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect distinct_action_types rows 失败")
    }

    /// 查询所有有记录的 (action_type, target_agent_id) 组合
    fn distinct_action_target_pairs(&self) -> anyhow::Result<Vec<(String, Option<String>)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("outcome memory lock poisoned: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT action_type, target_agent_id FROM outcome_records ORDER BY action_type",
            )
            .context("prepare distinct_action_target_pairs 失败")?;
        let rows = stmt
            .query_map([], |row| {
                let at: String = row.get(0)?;
                let target: Option<String> = row.get(1)?;
                Ok((at, target))
            })
            .context("execute distinct_action_target_pairs 失败")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect distinct_action_target_pairs rows 失败")
    }

    /// 生成 prompt 注入文本（按动作类型 + 交互对象聚合）
    ///
    /// P1-3 修复：query 失败时 warn! 记录后降级为空字符串，**不 panic**。
    /// 业务契约：memory 是 best-effort，DB 错时主流程（LLM prompt 构建）不能阻断。
    pub fn to_prompt_context(&self) -> String {
        let pairs = match self.distinct_action_target_pairs() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    "outcome memory distinct_action_target_pairs 失败，prompt 降级为空：{e:?}"
                );
                return String::new();
            }
        };
        let mut lines: Vec<String> = Vec::new();

        for (at, target) in &pairs {
            let records: Vec<OutcomeRecord> = match target {
                Some(tid) => match self.query_by_target(tid, self.prompt_limit) {
                    Ok(mut rs) => {
                        rs.retain(|r| r.action_type == *at);
                        rs
                    }
                    Err(e) => {
                        tracing::warn!(
                            "outcome memory query_by_target({tid}) 失败，跳过此对：{e:?}"
                        );
                        continue;
                    }
                },
                None => match self.query_recent(at, self.prompt_limit) {
                    Ok(mut rs) => {
                        rs.retain(|r| r.target_agent_id.is_none());
                        rs
                    }
                    Err(e) => {
                        tracing::warn!(
                            "outcome memory query_recent({at}) 失败，跳过此 action：{e:?}"
                        );
                        continue;
                    }
                },
            };
            if records.is_empty() {
                continue;
            }
            let label = match target {
                Some(tid) => format!("{} {}", at, tid),
                None => at.clone(),
            };
            let success_count = records
                .iter()
                .filter(|r| matches!(r.result, OutcomeResult::Success))
                .count();
            let fail_count = records.len() - success_count;

            if success_count > 0 && fail_count > 0 {
                lines.push(format!(
                    "- {} → 成功{}次/失败{}次",
                    label, success_count, fail_count
                ));
            } else if success_count > 0 {
                lines.push(format!("- {} → 成功 [{}次]", label, success_count));
            } else if fail_count > 0
                && let Some(reason) = records.iter().find_map(|r| match &r.result {
                    OutcomeResult::Failed(r) => Some(r.clone()),
                    _ => None,
                })
            {
                lines.push(format!(
                    "- {} → 失败（{}）[{}次]",
                    label, reason, fail_count
                ));
            }
        }

        if lines.is_empty() {
            return String::new();
        }
        format!("\n### 经验教训\n{}\n", lines.join("\n"))
    }

    /// 生成 prompt 注入文本（旧版，仅按动作类型聚合）
    #[allow(dead_code)]
    fn to_prompt_context_by_action(&self) -> String {
        // P1-3 修复：与新 to_prompt_context 相同的 best-effort 降级策略。
        let action_types = match self.distinct_action_types() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    "outcome memory distinct_action_types 失败，prompt 降级为空：{e:?}"
                );
                return String::new();
            }
        };
        let mut lines: Vec<String> = Vec::new();

        for at in &action_types {
            let records = match self.query_recent(at, self.prompt_limit) {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        "outcome memory query_recent({at}) 失败，跳过：{e:?}"
                    );
                    continue;
                }
            };
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
                let short_reason = reason.clone();
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
            Err(e) => {
                tracing::warn!("outcome memory cleanup: lock poisoned（best-effort 跳过本轮清理）：{e:?}");
                return;
            }
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
            action_type: "用".into(),
            action_data: Some(serde_json::json!({"item_id": "馒头"})),
            result: OutcomeResult::Success,
            target_agent_id: None,
            context_hash: "龙门大堂:food,drink:2".into(),
            tick_id: 100,
        })
        .expect("record must succeed in test");

        mem.record(OutcomeRecord {
            action_type: "用".into(),
            action_data: Some(serde_json::json!({"item_id": "invalid"})),
            result: OutcomeResult::Failed("物品不存在".into()),
            target_agent_id: None,
            context_hash: "龙门大堂:food,drink:2".into(),
            tick_id: 101,
        })
        .expect("record must succeed in test");

        let records = mem.query_recent("用", 10).expect("query_recent in test");
        assert_eq!(records.len(), 2);
        assert!(matches!(records[0].result, OutcomeResult::Failed(_)));
        assert!(matches!(records[1].result, OutcomeResult::Success));

        let rate = mem.success_rate("用").expect("success_rate in test");
        assert!((rate - 0.5).abs() < 0.01);

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn test_prompt_context() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).unwrap();

        mem.record(OutcomeRecord {
            action_type: "移动".into(),
            action_data: Some(serde_json::json!({"target_location": "龙门厨房"})),
            result: OutcomeResult::Success,
            target_agent_id: None,
            context_hash: "龙门大堂::1".into(),
            tick_id: 100,
        })
        .expect("record must succeed in test");

        let ctx = mem.to_prompt_context();
        assert!(ctx.contains("经验教训"));
        assert!(ctx.contains("移动 → 成功"));

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn test_dynamic_action_types() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).unwrap();

        mem.record(OutcomeRecord {
            action_type: "攻击".into(),
            action_data: None,
            result: OutcomeResult::Success,
            target_agent_id: None,
            context_hash: "loc::0".into(),
            tick_id: 100,
        })
        .expect("record must succeed in test");

        let types = mem.distinct_action_types().expect("distinct_action_types in test");
        assert!(types.contains(&"攻击".to_string()));

        let ctx = mem.to_prompt_context();
        assert!(ctx.contains("攻击 → 成功"));

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn test_query_by_target() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).unwrap();

        let target_id = "agent-b";
        mem.record(OutcomeRecord {
            action_type: "予".into(),
            action_data: Some(serde_json::json!({"item_id": "馒头", "quantity": 10, "target_agent_id": target_id})),
            result: OutcomeResult::Success,
            target_agent_id: Some(target_id.to_string()),
            context_hash: "loc::1".into(),
            tick_id: 100,
        })
        .expect("record must succeed in test");
        mem.record(OutcomeRecord {
            action_type: "予".into(),
            action_data: None,
            result: OutcomeResult::Success,
            target_agent_id: Some(target_id.to_string()),
            context_hash: "loc::1".into(),
            tick_id: 101,
        })
        .expect("record must succeed in test");
        mem.record(OutcomeRecord {
            action_type: "攻击".into(),
            action_data: None,
            result: OutcomeResult::Success,
            target_agent_id: Some("agent-c".to_string()),
            context_hash: "loc::1".into(),
            tick_id: 102,
        })
        .expect("record must succeed in test");

        let records = mem
            .query_by_target(target_id, 10)
            .expect("query_by_target in test");
        assert_eq!(records.len(), 2);
        assert!(
            records
                .iter()
                .all(|r| r.target_agent_id.as_deref() == Some(target_id))
        );

        let records_c = mem
            .query_by_target("agent-c", 10)
            .expect("query_by_target in test");
        assert_eq!(records_c.len(), 1);

        let records_none = mem
            .query_by_target("nonexistent", 10)
            .expect("query_by_target in test");
        assert!(records_none.is_empty());

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn test_extract_target_agent_id() {
        assert_eq!(
            extract_target_agent_id(&Some(serde_json::json!({"target_agent_id": "abc"}))),
            Some("abc".to_string())
        );
        assert_eq!(
            extract_target_agent_id(&Some(serde_json::json!({"target_id": "def"}))),
            Some("def".to_string())
        );
        assert_eq!(
            extract_target_agent_id(&Some(serde_json::json!({"target_uuid": "ghi"}))),
            Some("ghi".to_string())
        );
        assert_eq!(
            extract_target_agent_id(&Some(serde_json::json!({"item_id": "馒头"}))),
            None
        );
        assert_eq!(extract_target_agent_id(&None), None);
    }

    #[test]
    fn test_prompt_context_per_target() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).unwrap();

        // 有 target 的动作
        mem.record(OutcomeRecord {
            action_type: "予".into(),
            action_data: None,
            result: OutcomeResult::Success,
            target_agent_id: Some("npc-a".to_string()),
            context_hash: "loc::1".into(),
            tick_id: 100,
        })
        .expect("record must succeed in test");
        mem.record(OutcomeRecord {
            action_type: "予".into(),
            action_data: None,
            result: OutcomeResult::Failed("物品不足".into()),
            target_agent_id: Some("npc-a".to_string()),
            context_hash: "loc::1".into(),
            tick_id: 101,
        })
        .expect("record must succeed in test");
        mem.record(OutcomeRecord {
            action_type: "予".into(),
            action_data: None,
            result: OutcomeResult::Success,
            target_agent_id: Some("npc-b".to_string()),
            context_hash: "loc::1".into(),
            tick_id: 102,
        })
        .expect("record must succeed in test");
        // 无 target 的动作
        mem.record(OutcomeRecord {
            action_type: "用".into(),
            action_data: None,
            result: OutcomeResult::Success,
            target_agent_id: None,
            context_hash: "loc::1".into(),
            tick_id: 103,
        })
        .expect("record must succeed in test");

        let ctx = mem.to_prompt_context();
        assert!(
            ctx.contains("予 npc-a"),
            "should contain per-target line: {}",
            ctx
        );
        assert!(
            ctx.contains("予 npc-b"),
            "should contain per-target line: {}",
            ctx
        );
        assert!(
            ctx.contains("用"),
            "should contain no-target action: {}",
            ctx
        );
        assert!(
            !ctx.contains("予 →"),
            "should NOT contain action-only line: {}",
            ctx
        );

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn test_outcome_memory_migrates_legacy_db_without_target_column() {
        // 模拟老库：只建表，缺 target_agent_id 列
        let db = temp_db();
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE outcome_records (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                action_type      TEXT NOT NULL,
                action_data      TEXT,
                result_type      TEXT NOT NULL,
                result           TEXT,
                context_hash     TEXT NOT NULL,
                tick_id          INTEGER NOT NULL,
                created_at       INTEGER DEFAULT (strftime('%s', 'now'))
            )",
        )
        .unwrap();
        drop(conn);

        // 旧版 OutcomeMemory 初始化会"成功"地假装完成迁移；新版必须真正补列
        let _mem = OutcomeMemory::new(&db, 10).expect("legacy migration should succeed");
        drop(_mem);
        let conn = Connection::open(&db).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(outcome_records)").unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut has_target = false;
        while let Some(row) = rows.next().unwrap() {
            let name: String = row.get(1).unwrap();
            if name == "target_agent_id" {
                has_target = true;
            }
        }
        assert!(
            has_target,
            "legacy DB should be migrated to include target_agent_id"
        );
        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn test_outcome_memory_init_is_idempotent_for_already_migrated_db() {
        // 全新建库、再 init、再 init：第二次不应失败
        let db = temp_db();
        let _first = OutcomeMemory::new(&db, 10).expect("first init");
        let _second = OutcomeMemory::new(&db, 10).expect("second init must be idempotent");
        let _ = std::fs::remove_file(&db);
    }

    // ========================================================================
    // P1-3 闭环：silent error visibility 测试
    // 测试方法：先 init OutcomeMemory（建表），再从外部 DROP TABLE，
    // 让 cached conn 的下个 query 失败（"no such table"）。
    // 修复前：返回空 Vec / 0.0 / ()，不报错 → 静默吞错
    // 修复后：返回 Err，由 caller 决定如何处理
    // ========================================================================

    fn break_db_by_dropping_table(db: &std::path::Path) {
        let conn = rusqlite::Connection::open(db).expect("reopen db to break it");
        conn.execute("DROP TABLE outcome_records", [])
            .expect("drop outcome_records from underneath");
    }

    /// 验证 P1-3：record() 必须返回 Result，DB 错时返回 Err。
    /// 之前用 `if let Err(e) = ... { debug!(...) }` 静默吞错，且 debug 级
    /// 默认不输出，运维完全看不到。
    #[test]
    fn test_p1_3_record_returns_err_on_broken_db() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).expect("new");
        break_db_by_dropping_table(&db);

        let result = mem.record(OutcomeRecord {
            action_type: "test".into(),
            action_data: None,
            result: OutcomeResult::Success,
            target_agent_id: None,
            context_hash: "ctx".into(),
            tick_id: 1,
        });
        assert!(
            result.is_err(),
            "P1-3 修复缺失：record() 在 DB 损坏时必须返回 Err，而非静默吞错。\
             当前 is_ok={}",
            result.is_ok()
        );
        let _ = std::fs::remove_file(&db);
    }

    /// 验证 P1-3：query_recent() 必须返回 Result，DB 错时返回 Err。
    /// 之前返回空 Vec 会让 caller 误以为"无记录"而非"DB 坏"——这是误导。
    #[test]
    fn test_p1_3_query_recent_returns_err_on_broken_db() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).expect("new");
        break_db_by_dropping_table(&db);

        let result = mem.query_recent("test", 10);
        assert!(
            result.is_err(),
            "P1-3 修复缺失：query_recent() 在 DB 损坏时必须返回 Err，\
             而非静默返回空 Vec（会让 caller 把 DB 错当成'无记录'）"
        );
        let _ = std::fs::remove_file(&db);
    }

    /// 验证 P1-3：query_by_target() 必须返回 Result。
    #[test]
    fn test_p1_3_query_by_target_returns_err_on_broken_db() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).expect("new");
        break_db_by_dropping_table(&db);

        let result = mem.query_by_target("target", 10);
        assert!(
            result.is_err(),
            "P1-3 修复缺失：query_by_target() 在 DB 损坏时必须返回 Err"
        );
        let _ = std::fs::remove_file(&db);
    }

    /// 验证 P1-3：success_rate() 必须返回 Result，DB 错时返回 Err。
    /// 之前返回 0.0 会让 caller 误以为"零成功率"而非"DB 错"——这是误导。
    #[test]
    fn test_p1_3_success_rate_returns_err_on_broken_db() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).expect("new");
        break_db_by_dropping_table(&db);

        let result = mem.success_rate("test");
        assert!(
            result.is_err(),
            "P1-3 修复缺失：success_rate() 在 DB 损坏时必须返回 Err，\
             而非静默返回 0.0（会把 DB 错当成'全失败'）"
        );
        let _ = std::fs::remove_file(&db);
    }

    /// 验证 P1-3：to_prompt_context() 必须在 query 失败时降级为空字符串（或部分内容），
    /// **不 panic** 且不 block 调用方。caller 期望的契约是 best-effort：
    /// DB 错 → 空内容 + 警告日志，而非 panic / 阻断主流程。
    #[test]
    fn test_p1_3_to_prompt_context_returns_empty_on_broken_db_without_panic() {
        let db = temp_db();
        let mem = OutcomeMemory::new(&db, 10).expect("new");
        break_db_by_dropping_table(&db);

        // to_prompt_context 的签名不变（仍返回 String），但内部必须显式处理
        // query 失败：warn! 记录后返回空或部分内容，**不 panic**。
        let ctx = mem.to_prompt_context();
        // 行为契约：返回空字符串（无 records 字段）
        assert!(
            ctx.is_empty() || !ctx.contains("record:"),
            "P1-3 修复后，to_prompt_context 在 DB 损坏时必须降级为空/部分内容，\
             不 panic 且无假数据。当前返回：{ctx}"
        );
        let _ = std::fs::remove_file(&db);
    }
}
