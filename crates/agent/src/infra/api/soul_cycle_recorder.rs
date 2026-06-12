// ============================================================================
// Soul Cycle Recorder - 三魂循环完整链路记录
// ============================================================================
//
// 记录每个 Tick 的三魂循环完整中间状态：
// - 人魂输出（结构化 Intent）
// - 天魂三层审查结果（layer1/2/3 各独立结果）
// - 即时通道说话意图
//
// 数据驱动：按 tick_id + attempt 隔离，同一 tick 重提交时覆盖。
// 存储后端：SQLite，按 agent_id 隔离（per-agent 数据库文件）

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// 三魂循环记录条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SoulCycleRecord {
    pub id: i64,
    pub tick_id: i64,
    pub attempt: i32,
    pub renhun_narrative: Option<String>,
    pub renhun_thought_log: Option<String>,
    pub tianhun_result: Option<String>,
    pub tianhun_layer1_result: Option<String>,
    pub tianhun_layer2_result: Option<String>,
    pub tianhun_layer3_result: Option<String>,
    pub tianhun_reason: Option<String>,
    pub previous_round_narrative: Option<String>,
    pub final_intent_id: Option<String>,
    pub final_action_type: Option<String>,
    pub final_action_data: Option<String>,
    pub final_pipeline_json: Option<String>,
    pub route_type: String,
    pub world_time: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 即时意图记录条目
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImmediateIntentRecord {
    pub id: i64,
    pub tick_id: i64,
    pub intent_id: String,
    pub source_narrative: Option<String>,
    pub route_type: String,
    pub action_type: String,
    pub action_data: Option<String>,
    pub speech_content: Option<String>,
    pub send_status: String,
    pub send_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 构建 SQLite IN 子句占位符：`build_in_placeholders(3)` → `"?1,?2,?3"`
fn build_in_placeholders(count: usize) -> String {
    (1..=count)
        .map(|i| format!("?{}", i))
        .collect::<Vec<_>>()
        .join(",")
}

/// 三魂循环记录器（SQLite 持久化）
///
/// 按 agent_id 隔离，使用独立的 SQLite 文件。
#[derive(Debug, Clone)]
pub struct SoulCycleRecorder {
    conn: Arc<Mutex<Connection>>,
}

impl SoulCycleRecorder {
    /// 打开或创建三魂循环记录器
    pub fn open(_agent_id: Uuid, db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create database directory")?;
        }

        let conn = Connection::open(db_path).context("Failed to open soul cycle database")?;
        Self::init_schema(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS soul_cycle_record (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tick_id INTEGER NOT NULL,
                attempt INTEGER NOT NULL DEFAULT 0,
                renhun_narrative TEXT,
                renhun_thought_log TEXT,
                tianhun_result TEXT,
                tianhun_layer1_result TEXT,
                tianhun_layer2_result TEXT,
                tianhun_layer3_result TEXT,
                tianhun_reason TEXT,
                previous_round_narrative TEXT,
                final_intent_id TEXT,
                final_action_type TEXT,
                final_action_data TEXT,
                final_pipeline_json TEXT,
                route_type TEXT NOT NULL DEFAULT 'main',
                world_time TEXT,
                created_at TEXT NOT NULL,
                UNIQUE(tick_id, attempt)
            )",
            [],
        )
        .context("Failed to create soul_cycle_record table")?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS immediate_intent_record (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tick_id INTEGER NOT NULL,
                intent_id TEXT NOT NULL,
                source_narrative TEXT,
                route_type TEXT NOT NULL,
                action_type TEXT NOT NULL,
                action_data TEXT,
                speech_content TEXT,
                send_status TEXT NOT NULL DEFAULT 'sent',
                send_error TEXT,
                created_at TEXT NOT NULL
            )",
            [],
        )
        .context("Failed to create immediate_intent_record table")?;

        conn.execute("PRAGMA journal_mode = WAL", []).ok();
        conn.execute("PRAGMA synchronous = NORMAL", []).ok();

        // idempotent migration: add cache_hit_rate column (ignore if already exists)
        conn.execute_batch("ALTER TABLE soul_cycle_record ADD COLUMN cache_hit_rate REAL")
            .ok();
        // idempotent migration: add final_pipeline_json column
        conn.execute_batch("ALTER TABLE soul_cycle_record ADD COLUMN final_pipeline_json TEXT")
            .ok();

        Ok(())
    }

    /// 记录人魂输出
    pub async fn record_renhun(
        &self,
        tick_id: i64,
        attempt: i32,
        narrative: &str,
        thought_log: &str,
    ) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");
        let created_at = Utc::now().to_rfc3339();

        let result = conn.execute(
            "INSERT INTO soul_cycle_record
             (tick_id, attempt, renhun_narrative, renhun_thought_log, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(tick_id, attempt) DO UPDATE SET
                renhun_narrative = excluded.renhun_narrative,
                renhun_thought_log = excluded.renhun_thought_log,
                created_at = excluded.created_at",
            params![tick_id, attempt, narrative, thought_log, created_at],
        );

        match result {
            Ok(_) => tracing::debug!(
                "[soul_cycle] Recorded renhun for tick {} attempt {}",
                tick_id,
                attempt
            ),
            Err(e) => tracing::warn!(
                "[soul_cycle] Failed to record renhun for tick {}: {}",
                tick_id,
                e
            ),
        }
    }

    /// 记录天魂审查结果
    #[allow(clippy::too_many_arguments)]
    pub async fn record_tianhun(
        &self,
        tick_id: i64,
        attempt: i32,
        result: &str,
        layer1: Option<&str>,
        layer2: Option<&str>,
        layer3: Option<&str>,
        reason: Option<&str>,
    ) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");
        let created_at = Utc::now().to_rfc3339();

        // 注意: previous_round_narrative 由 update_previous_round_narrative() 独占管理
        // 此处不再写入该列，避免当轮审批叙事覆盖上轮执行叙事
        let result = conn.execute(
            "UPDATE soul_cycle_record SET
                tianhun_result = ?1,
                tianhun_layer1_result = ?2,
                tianhun_layer2_result = ?3,
                tianhun_layer3_result = ?4,
                tianhun_reason = ?5,
                created_at = ?6
             WHERE tick_id = ?7 AND attempt = ?8",
            params![
                result, layer1, layer2, layer3, reason, created_at, tick_id, attempt
            ],
        );

        match result {
            Ok(n) if n > 0 => tracing::debug!(
                "[soul_cycle] Recorded tianhun for tick {} attempt {}",
                tick_id,
                attempt
            ),
            Ok(_) => tracing::warn!(
                "[soul_cycle] No record found for tick {} attempt {} when recording tianhun",
                tick_id,
                attempt
            ),
            Err(e) => tracing::warn!(
                "[soul_cycle] Failed to record tianhun for tick {}: {}",
                tick_id,
                e
            ),
        }
    }

    /// 更新 previous_round_narrative（在感知阶段生成后回填）
    ///
    /// 用于将天魂生成的执行叙事回填到上一轮的 soul_cycle_record。
    pub async fn update_previous_round_narrative(&self, tick_id: i64, narrative: &str) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");

        let result = conn.execute(
            "UPDATE soul_cycle_record SET previous_round_narrative = ?1 WHERE tick_id = ?2",
            params![narrative, tick_id],
        );

        match result {
            Ok(n) if n > 0 => tracing::debug!(
                "[soul_cycle] Updated previous_round_narrative for tick {}",
                tick_id
            ),
            Ok(_) => tracing::warn!(
                "[soul_cycle] No record found for tick {} when updating previous_round_narrative",
                tick_id
            ),
            Err(e) => tracing::warn!(
                "[soul_cycle] Failed to update previous_round_narrative for tick {}: {}",
                tick_id,
                e
            ),
        }
    }

    /// 记录最终 Intent
    pub async fn record_final_intent(
        &self,
        tick_id: i64,
        attempt: i32,
        intent_id: Option<&str>,
        action_type: Option<&str>,
        action_data: Option<&str>,
        pipeline_json: Option<&str>,
    ) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");
        let created_at = Utc::now().to_rfc3339();

        let result = conn.execute(
            "UPDATE soul_cycle_record SET
                final_intent_id = ?1,
                final_action_type = ?2,
                final_action_data = ?3,
                final_pipeline_json = ?4,
                created_at = ?5
             WHERE tick_id = ?6 AND attempt = ?7",
            params![
                intent_id,
                action_type,
                action_data,
                pipeline_json,
                created_at,
                tick_id,
                attempt
            ],
        );

        match result {
            Ok(n) if n > 0 => tracing::debug!(
                "[soul_cycle] Recorded final_intent for tick {} attempt {}",
                tick_id,
                attempt
            ),
            Ok(_) => tracing::warn!(
                "[soul_cycle] No record found for tick {} attempt {} when recording final_intent",
                tick_id,
                attempt
            ),
            Err(e) => tracing::warn!(
                "[soul_cycle] Failed to record final_intent for tick {}: {}",
                tick_id,
                e
            ),
        }
    }

    /// 更新 world_time（可选，tick_id 已可关联 WorldState，此字段为便利数据）
    pub async fn record_world_time(&self, tick_id: i64, attempt: i32, world_time: &str) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");

        let result = conn.execute(
            "UPDATE soul_cycle_record SET world_time = ?1 WHERE tick_id = ?2 AND attempt = ?3",
            params![world_time, tick_id, attempt],
        );

        if result.is_err() {
            tracing::warn!(
                "[soul_cycle] Failed to record world_time for tick {}",
                tick_id
            );
        }
    }

    /// 记录即时通道意图
    #[allow(clippy::too_many_arguments)]
    pub async fn record_immediate(
        &self,
        tick_id: i64,
        intent_id: &str,
        source_narrative: Option<&str>,
        route_type: &str,
        action_type: &str,
        action_data: Option<&str>,
        speech_content: Option<&str>,
        send_status: &str,
        send_error: Option<&str>,
    ) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");
        let created_at = Utc::now().to_rfc3339();

        let result = conn.execute(
            "INSERT INTO immediate_intent_record
             (tick_id, intent_id, source_narrative, route_type, action_type, action_data, speech_content, send_status, send_error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                tick_id,
                intent_id,
                source_narrative,
                route_type,
                action_type,
                action_data,
                speech_content,
                send_status,
                send_error,
                created_at
            ],
        );

        match result {
            Ok(_) => tracing::debug!(
                "[soul_cycle] Recorded immediate intent for tick {}",
                tick_id
            ),
            Err(e) => tracing::warn!(
                "[soul_cycle] Failed to record immediate intent for tick {}: {}",
                tick_id,
                e
            ),
        }
    }

    /// 获取小于指定 tick_id 的最近一个有记录的 tick_id
    ///
    /// Agent 推理频率低于 tick 推进频率，tick_id 不连续。
    /// 用于 `update_previous_round_narrative` 找到真正需要回填的上一轮 tick。
    pub async fn get_last_recorded_tick(&self, before_tick_id: i64) -> Option<i64> {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return None,
        };

        conn.query_row(
            "SELECT MAX(tick_id) FROM soul_cycle_record WHERE tick_id < ?1",
            params![before_tick_id],
            |row| row.get(0),
        )
        .ok()
        .flatten()
    }

    /// 获取上轮人魂叙事（最近一条 tick_id < before_tick_id 的记录）
    pub async fn get_last_renhun_narrative(&self, before_tick_id: i64) -> Option<String> {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return None,
        };

        // 取最近一条成功的记录（attempt 最大的，通常是最终通过的）
        conn.query_row(
            "SELECT renhun_narrative FROM soul_cycle_record WHERE tick_id < ?1 AND renhun_narrative IS NOT NULL ORDER BY tick_id DESC, attempt DESC LIMIT 1",
            params![before_tick_id],
            |row| row.get(0),
        )
        .ok()
        .flatten()
    }

    /// 按 tick_id 获取所有 attempt 的记录
    pub async fn get_by_tick(&self, tick_id: i64) -> Vec<SoulCycleRecord> {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return vec![],
        };

        let mut stmt = match conn.prepare(
            "SELECT id, tick_id, attempt, renhun_narrative, renhun_thought_log,
                    tianhun_result, tianhun_layer1_result, tianhun_layer2_result,
                    tianhun_layer3_result, tianhun_reason, previous_round_narrative,
                    final_intent_id, final_action_type, final_action_data, final_pipeline_json,
                    route_type, world_time, created_at
             FROM soul_cycle_record WHERE tick_id = ?1 ORDER BY attempt ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        stmt.query_map(params![tick_id], |row| Ok(Self::row_to_record(row)))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 按 tick 分组的分页查询（返回去重 tick_id 列表）
    pub async fn get_tick_ids_page(&self, page: u32, limit: u32) -> (Vec<i64>, u32) {
        let page = page.max(1);
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return (vec![], 0),
        };

        let total: u32 = conn
            .query_row(
                "SELECT COUNT(DISTINCT tick_id) FROM soul_cycle_record",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let offset = ((page - 1) * limit) as i64;
        let mut stmt = match conn.prepare(
            "SELECT DISTINCT tick_id FROM soul_cycle_record ORDER BY tick_id DESC LIMIT ?1 OFFSET ?2",
        ) {
            Ok(s) => s,
            Err(_) => return (vec![], total),
        };

        let tick_ids: Vec<i64> = stmt
            .query_map(params![limit, offset], |row| row.get(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        (tick_ids, total)
    }

    /// 批量获取多个 tick 的三魂循环记录（单次 SQL，消除 N+1）
    pub async fn get_by_ticks(&self, tick_ids: &[i64]) -> Vec<SoulCycleRecord> {
        if tick_ids.is_empty() || tick_ids.len() > 100 {
            return vec![];
        }
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return vec![],
        };

        let sql = format!(
            "SELECT id, tick_id, attempt, renhun_narrative, renhun_thought_log,
                    tianhun_result, tianhun_layer1_result, tianhun_layer2_result,
                    tianhun_layer3_result, tianhun_reason, previous_round_narrative,
                    final_intent_id, final_action_type, final_action_data, final_pipeline_json,
                    route_type, world_time, created_at
             FROM soul_cycle_record WHERE tick_id IN ({}) ORDER BY tick_id DESC, attempt ASC",
            build_in_placeholders(tick_ids.len())
        );

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let params: Vec<&dyn rusqlite::ToSql> = tick_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        stmt.query_map(params.as_slice(), |row| Ok(Self::row_to_record(row)))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// 批量获取多个 tick 的即时意图记录
    pub async fn get_immediate_by_ticks(&self, tick_ids: &[i64]) -> Vec<ImmediateIntentRecord> {
        if tick_ids.is_empty() || tick_ids.len() > 100 {
            return vec![];
        }
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return vec![],
        };

        let sql = format!(
            "SELECT id, tick_id, intent_id, source_narrative, route_type,
                    action_type, action_data, speech_content, send_status, send_error, created_at
             FROM immediate_intent_record WHERE tick_id IN ({}) ORDER BY id ASC",
            build_in_placeholders(tick_ids.len())
        );

        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        // 绑定参数
        let params: Vec<&dyn rusqlite::ToSql> = tick_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        stmt.query_map(params.as_slice(), |row| {
            Ok(Self::row_to_immediate_record(row))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// 获取即时意图记录
    pub async fn get_immediate_by_tick(&self, tick_id: i64) -> Vec<ImmediateIntentRecord> {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return vec![],
        };

        let mut stmt = match conn.prepare(
            "SELECT id, tick_id, intent_id, source_narrative, route_type,
                    action_type, action_data, speech_content, send_status, send_error, created_at
             FROM immediate_intent_record WHERE tick_id = ?1 ORDER BY id ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        stmt.query_map(params![tick_id], |row| {
            Ok(Self::row_to_immediate_record(row))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    fn row_to_record(row: &rusqlite::Row<'_>) -> SoulCycleRecord {
        let created_at_str: String = row.get(17).unwrap_or_default();
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        SoulCycleRecord {
            id: row.get(0).unwrap_or(0),
            tick_id: row.get(1).unwrap_or(0),
            attempt: row.get(2).unwrap_or(0),
            renhun_narrative: row.get(3).ok(),
            renhun_thought_log: row.get(4).ok(),
            tianhun_result: row.get(5).ok(),
            tianhun_layer1_result: row.get(6).ok(),
            tianhun_layer2_result: row.get(7).ok(),
            tianhun_layer3_result: row.get(8).ok(),
            tianhun_reason: row.get(9).ok(),
            previous_round_narrative: row.get(10).ok(),
            final_intent_id: row.get(11).ok(),
            final_action_type: row.get(12).ok(),
            final_action_data: row.get(13).ok(),
            final_pipeline_json: row.get(14).ok(),
            route_type: row.get(15).unwrap_or_else(|_| "main".to_string()),
            world_time: row.get(16).ok(),
            created_at,
        }
    }

    fn row_to_immediate_record(row: &rusqlite::Row<'_>) -> ImmediateIntentRecord {
        let created_at_str: String = row.get(10).unwrap_or_default();
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        ImmediateIntentRecord {
            id: row.get(0).unwrap_or(0),
            tick_id: row.get(1).unwrap_or(0),
            intent_id: row.get(2).unwrap_or_default(),
            source_narrative: row.get(3).ok(),
            route_type: row.get(4).unwrap_or_default(),
            action_type: row.get(5).unwrap_or_default(),
            action_data: row.get(6).ok(),
            speech_content: row.get(7).ok(),
            send_status: row.get(8).unwrap_or_else(|_| "sent".to_string()),
            send_error: row.get(9).ok(),
            created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_recorder() -> (TempDir, SoulCycleRecorder) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("soul_cycle.db");
        let recorder = SoulCycleRecorder::open(Uuid::new_v4(), &db_path).unwrap();
        (temp_dir, recorder)
    }

    #[tokio::test]
    async fn test_record_renhun() {
        let (_dir, recorder) = make_recorder();
        recorder
            .record_renhun(1, 0, "吃馒头充饥", "思考中...")
            .await;
        let records = recorder.get_by_tick(1).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].renhun_narrative.as_deref(), Some("吃馒头充饥"));
        assert_eq!(records[0].renhun_thought_log.as_deref(), Some("思考中..."));
    }

    #[tokio::test]
    async fn test_record_tianhun_approved() {
        let (_dir, recorder) = make_recorder();
        recorder.record_renhun(1, 0, "吃馒头", "...").await;
        recorder
            .record_tianhun(
                1,
                0,
                "approved",
                Some("action_type合法"),
                Some("物品存在"),
                None,
                None,
            )
            .await;
        let records = recorder.get_by_tick(1).await;
        assert_eq!(records[0].tianhun_result.as_deref(), Some("approved"));
        assert_eq!(
            records[0].tianhun_layer1_result.as_deref(),
            Some("action_type合法")
        );
    }

    #[tokio::test]
    async fn test_record_tianhun_rejected() {
        let (_dir, recorder) = make_recorder();
        recorder.record_renhun(1, 0, "无效", "...").await;
        recorder
            .record_tianhun(
                1,
                0,
                "rejected",
                Some("action_type合法"),
                None,
                None,
                Some("意图不合理"),
            )
            .await;
        let records = recorder.get_by_tick(1).await;
        assert_eq!(records[0].tianhun_result.as_deref(), Some("rejected"));
        assert_eq!(records[0].tianhun_reason.as_deref(), Some("意图不合理"));
    }

    #[tokio::test]
    async fn test_unique_constraint_tick_attempt() {
        let (_dir, recorder) = make_recorder();
        recorder.record_renhun(1, 0, "第一次", "...").await;
        recorder.record_renhun(1, 0, "第二次覆盖", "...").await;
        let records = recorder.get_by_tick(1).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].renhun_narrative.as_deref(), Some("第二次覆盖"));
    }

    #[tokio::test]
    async fn test_record_immediate() {
        let (_dir, recorder) = make_recorder();
        recorder
            .record_immediate(
                1,
                "uuid123",
                Some("和人打招呼"),
                "extracted",
                "说话",
                Some(r#"{"content":"你好"}"#),
                Some("你好"),
                "sent",
                None,
            )
            .await;
        let records = recorder.get_immediate_by_tick(1).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].action_type, "说话");
        assert_eq!(records[0].send_status, "sent");
    }

    #[tokio::test]
    async fn test_record_immediate_failed() {
        let (_dir, recorder) = make_recorder();
        recorder
            .record_immediate(
                1,
                "uuid456",
                Some("喊话"),
                "pure",
                "说话",
                Some(r#"{"content":"救命"}"#),
                Some("救命"),
                "failed",
                Some("WebSocket 断开"),
            )
            .await;
        let records = recorder.get_immediate_by_tick(1).await;
        assert_eq!(records[0].send_status, "failed");
        assert_eq!(records[0].send_error.as_deref(), Some("WebSocket 断开"));
    }

    #[tokio::test]
    async fn test_world_time() {
        let (_dir, recorder) = make_recorder();
        recorder.record_renhun(1, 0, "移动", "...").await;
        recorder.record_world_time(1, 0, "第三天 申时").await;
        let records = recorder.get_by_tick(1).await;
        assert_eq!(records[0].world_time.as_deref(), Some("第三天 申时"));
    }

    #[tokio::test]
    async fn test_get_tick_ids_page_dedup_and_order() {
        let (_dir, recorder) = make_recorder();
        // tick 1 有 2 次 attempt，tick 2 和 3 各 1 次
        recorder.record_renhun(1, 0, "a1", "...").await;
        recorder.record_renhun(1, 1, "a2", "...").await;
        recorder.record_renhun(3, 0, "c", "...").await;
        recorder.record_renhun(2, 0, "b", "...").await;

        let (ids, total) = recorder.get_tick_ids_page(1, 10).await;
        assert_eq!(total, 3);
        assert_eq!(ids, vec![3, 2, 1]); // 降序，tick 1 只出现一次
    }

    #[tokio::test]
    async fn test_get_tick_ids_page_pagination() {
        let (_dir, recorder) = make_recorder();
        for i in 1..=5 {
            recorder
                .record_renhun(i, 0, &format!("n{}", i), "...")
                .await;
        }

        let (p1, total) = recorder.get_tick_ids_page(1, 3).await;
        let (p2, _) = recorder.get_tick_ids_page(2, 3).await;
        assert_eq!(total, 5);
        assert_eq!(p1, vec![5, 4, 3]);
        assert_eq!(p2, vec![2, 1]);
    }

    #[tokio::test]
    async fn test_get_tick_ids_page_empty() {
        let (_dir, recorder) = make_recorder();
        let (ids, total) = recorder.get_tick_ids_page(1, 10).await;
        assert!(ids.is_empty());
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn test_get_by_ticks_batch() {
        let (_dir, recorder) = make_recorder();
        recorder.record_renhun(1, 0, "a", "...").await;
        recorder.record_renhun(1, 1, "a2", "...").await;
        recorder.record_renhun(3, 0, "c", "...").await;
        // tick 2 不存在

        let records = recorder.get_by_ticks(&[1, 2, 3]).await;
        assert_eq!(records.len(), 3); // tick1×2 + tick3×1
        assert_eq!(records[0].tick_id, 3); // 降序
        assert_eq!(records[1].tick_id, 1);
        assert_eq!(records[2].tick_id, 1);
    }

    #[tokio::test]
    async fn test_get_by_ticks_empty() {
        let (_dir, recorder) = make_recorder();
        let records = recorder.get_by_ticks(&[]).await;
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn test_get_immediate_by_ticks_batch() {
        let (_dir, recorder) = make_recorder();
        recorder
            .record_immediate(
                1,
                "id1",
                None,
                "extracted",
                "说话",
                None,
                Some("hi"),
                "sent",
                None,
            )
            .await;
        recorder
            .record_immediate(
                3,
                "id2",
                None,
                "pure",
                "说话",
                None,
                Some("bye"),
                "sent",
                None,
            )
            .await;
        recorder
            .record_immediate(
                3,
                "id3",
                None,
                "pure",
                "说话",
                None,
                Some("yo"),
                "failed",
                Some("err"),
            )
            .await;

        let records = recorder.get_immediate_by_ticks(&[1, 2, 3]).await;
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].tick_id, 1);
        assert_eq!(records[1].tick_id, 3);
        assert_eq!(records[2].tick_id, 3);
    }
}
