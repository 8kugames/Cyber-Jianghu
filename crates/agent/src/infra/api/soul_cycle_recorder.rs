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

use super::soul_cycle_types::build_in_placeholders;
pub use super::soul_cycle_types::{ImmediateIntentRecord, SoulCycleRecord};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// 超过时由 get_by_ticks / get_immediate_by_ticks 内部自动分批。
/// IN 查询单批 tick 上限（SQLite 默认变量数上限 999 的安全余量），
const TICK_BATCH_SIZE: usize = 100;

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
        // idempotent migration: add earth_tool_calls column (地魂 tool calling 日志)
        conn.execute_batch("ALTER TABLE soul_cycle_record ADD COLUMN earth_tool_calls TEXT")
            .ok();
        // idempotent migration: add model_id column (该次尝试使用的 LLM 模型 ID)
        conn.execute_batch("ALTER TABLE soul_cycle_record ADD COLUMN model_id TEXT")
            .ok();
        // idempotent migration: add tianhun_layers column (天魂审查结果 JSON 数组)
        conn.execute_batch("ALTER TABLE soul_cycle_record ADD COLUMN tianhun_layers TEXT")
            .ok();
        // idempotent migration: add server_execution_results column (Server 执行结果回填)
        conn.execute_batch(
            "ALTER TABLE soul_cycle_record ADD COLUMN server_execution_results TEXT",
        )
        .ok();

        Ok(())
    }

    /// 记录人魂输出
    ///
    /// `model_id` 收原始模型名（可为空串或占位符 "unknown"），归一后落库：
    /// 归一结果为空时写 NULL，使"未上报"在数据库中只有 NULL 一种表示。
    pub async fn record_renhun(
        &self,
        tick_id: i64,
        attempt: i32,
        narrative: &str,
        thought_log: &str,
        model_id: &str,
    ) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");
        let created_at = Utc::now().to_rfc3339();
        let model_id = crate::component::llm::normalize_model_id(model_id);

        let result = conn.execute(
            "INSERT INTO soul_cycle_record
             (tick_id, attempt, renhun_narrative, renhun_thought_log, model_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(tick_id, attempt) DO UPDATE SET
                renhun_narrative = excluded.renhun_narrative,
                renhun_thought_log = excluded.renhun_thought_log,
                model_id = excluded.model_id,
                route_type = 'main',
                created_at = excluded.created_at",
            params![
                tick_id,
                attempt,
                narrative,
                thought_log,
                model_id,
                created_at
            ],
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

    /// 记录天魂审查结果（layer0-3 全量落入 tianhun_layers JSON，DB 列仅存 layer1-3）
    ///
    /// layers_json：调用方预序列化的多意图聚合结构
    /// `[{"intent":"吃","layers":[{layer,passed,detail}]}]`；Some 时直接落入
    /// tianhun_layers 列（覆盖内部逐层构建），None 时保持旧的单意图格式。
    #[allow(clippy::too_many_arguments)]
    pub async fn record_tianhun(
        &self,
        tick_id: i64,
        attempt: i32,
        result: &str,
        layer0: Option<&str>,
        layer1: Option<&str>,
        layer2: Option<&str>,
        layer3: Option<&str>,
        reason: Option<&str>,
        layers_json: Option<&str>,
    ) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");
        let created_at = Utc::now().to_rfc3339();

        // 构建 tianhun_layers JSON 数组（数据驱动可扩展）；
        // 调用方传入聚合 JSON（多意图逐意图结果）时优先落库
        let tianhun_layers = if let Some(agg) = layers_json {
            Some(agg.to_string())
        } else {
            let layers: Vec<serde_json::Value> =
                [(0, layer0), (1, layer1), (2, layer2), (3, layer3)]
                    .into_iter()
                    .filter_map(|(idx, layer)| {
                        layer.map(|v| {
                            serde_json::json!({
                                "layer": format!("layer{}", idx),
                                "passed": v != "rejected",
                                "detail": if v == "rejected" { "驳回" } else { v },
                            })
                        })
                    })
                    .collect();
            if layers.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&layers).unwrap_or_default())
            }
        };

        // 注意: previous_round_narrative 由 update_previous_round_narrative() 独占管理
        // 此处不再写入该列，避免当轮审批叙事覆盖上轮执行叙事
        let result = conn.execute(
            "UPDATE soul_cycle_record SET
                tianhun_result = ?1,
                tianhun_layer1_result = ?2,
                tianhun_layer2_result = ?3,
                tianhun_layer3_result = ?4,
                tianhun_reason = ?5,
                tianhun_layers = ?6,
                created_at = ?7
             WHERE tick_id = ?8 AND attempt = ?9",
            params![
                result,
                layer1,
                layer2,
                layer3,
                reason,
                tianhun_layers,
                created_at,
                tick_id,
                attempt
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

    /// 追加天魂理由（保留既有内容，用于 chaos 覆写等后置替换的留痕）
    pub async fn append_tianhun_reason(&self, tick_id: i64, attempt: i32, note: &str) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");
        let result = conn.execute(
            "UPDATE soul_cycle_record SET
                tianhun_reason = CASE
                    WHEN tianhun_reason IS NULL OR tianhun_reason = '' THEN ?1
                    ELSE tianhun_reason || '；' || ?1
                END
             WHERE tick_id = ?2 AND attempt = ?3",
            params![note, tick_id, attempt],
        );
        match result {
            Ok(n) if n > 0 => tracing::debug!(
                "[soul_cycle] Appended tianhun reason for tick {} attempt {}",
                tick_id,
                attempt
            ),
            Ok(_) => tracing::warn!(
                "[soul_cycle] No record found for tick {} attempt {} when appending tianhun reason",
                tick_id,
                attempt
            ),
            Err(e) => tracing::warn!(
                "[soul_cycle] Failed to append tianhun reason for tick {}: {}",
                tick_id,
                e
            ),
        }
    }

    /// 回填 Server 执行结果（幂等，按 (tick_id, attempt) 更新 server_execution_results）
    pub async fn backfill_server_result(
        &self,
        tick_id: i64,
        attempt: i32,
        execution_results_json: &str,
    ) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");
        let result = conn.execute(
            "UPDATE soul_cycle_record SET server_execution_results = ?1 WHERE tick_id = ?2 AND attempt = ?3",
            params![execution_results_json, tick_id, attempt],
        );

        match result {
            Ok(n) if n > 0 => tracing::debug!(
                "[soul_cycle] Backfilled server results for tick {} attempt {}",
                tick_id,
                attempt
            ),
            Ok(_) => tracing::warn!(
                "[soul_cycle] No record found for tick {} attempt {} when backfilling",
                tick_id,
                attempt
            ),
            Err(e) => tracing::warn!(
                "[soul_cycle] Failed to backfill server results for tick {}: {}",
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

    /// 记录空转 tick 占位（route_type='idle_skip'，区别于真实认知循环的 'main'）
    ///
    /// 让三魂纪事时间轴连续：下游可凭 route_type 区分"无经历的空转"与"记录加载失败"。
    /// INSERT OR IGNORE：同一 tick 若已有真实认知记录则绝不覆盖。
    ///
    /// `model_id` 为该角色当前活跃模型名——空转 tick 本身不调用 LLM，此处上报的是
    /// 该角色的模型配置，使经历日志不再出现"有经历但无模型"的空列；"本 tick 未调用 LLM"
    /// 这一语义由 route_type='idle_skip' 承载，不占用模型字段。
    pub async fn record_idle_skip(
        &self,
        tick_id: i64,
        narrative: &str,
        world_time: Option<&str>,
        model_id: &str,
    ) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");
        let created_at = Utc::now().to_rfc3339();
        let model_id = crate::component::llm::normalize_model_id(model_id);

        let result = conn.execute(
            "INSERT OR IGNORE INTO soul_cycle_record
             (tick_id, attempt, renhun_narrative, route_type, world_time, model_id, created_at)
             VALUES (?1, 0, ?2, 'idle_skip', ?3, ?4, ?5)",
            params![tick_id, narrative, world_time, model_id, created_at],
        );

        match result {
            Ok(_) => tracing::debug!("[soul_cycle] Recorded idle_skip for tick {}", tick_id),
            Err(e) => tracing::warn!(
                "[soul_cycle] Failed to record idle_skip for tick {}: {}",
                tick_id,
                e
            ),
        }
    }

    /// 记录地魂 tool calling 日志
    pub async fn record_earth_tool_calls(&self, tick_id: i64, attempt: i32, tool_calls_json: &str) {
        let conn = self
            .conn
            .lock()
            .expect("soul_cycle_recorder lock not poisoned");

        let result = conn.execute(
            "UPDATE soul_cycle_record SET earth_tool_calls = ?1 WHERE tick_id = ?2 AND attempt = ?3",
            params![tool_calls_json, tick_id, attempt],
        );

        match result {
            Ok(n) if n > 0 => tracing::debug!(
                "[soul_cycle] Recorded earth_tool_calls for tick {} attempt {}",
                tick_id,
                attempt
            ),
            Ok(_) => tracing::warn!(
                "[soul_cycle] No record found for tick {} attempt {} when recording earth_tool_calls",
                tick_id,
                attempt
            ),
            Err(e) => tracing::warn!(
                "[soul_cycle] Failed to record earth_tool_calls for tick {}: {}",
                tick_id,
                e
            ),
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
    pub async fn get_last_recorded_tick(&self, before_tick_id: i64) -> anyhow::Result<Option<i64>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("soul_cycle_record lock poisoned: {e}"))?;
        conn.query_row(
            "SELECT MAX(tick_id) FROM soul_cycle_record WHERE tick_id < ?1",
            params![before_tick_id],
            |row| row.get(0),
        )
        .context("get_last_recorded_tick query 失败")
    }

    /// 获取上轮人魂叙事（最近一条 tick_id < before_tick_id 的记录）
    pub async fn get_last_renhun_narrative(
        &self,
        before_tick_id: i64,
    ) -> anyhow::Result<Option<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("soul_cycle_record lock poisoned: {e}"))?;
        // 取最近一条成功的记录（attempt 最大的，通常是最终通过的）；
        // 排除 idle_skip 占位行——"你上一轮的行动"提示不得把空转占位当作真实行动
        conn.query_row(
            "SELECT renhun_narrative FROM soul_cycle_record WHERE tick_id < ?1 AND renhun_narrative IS NOT NULL AND route_type = 'main' ORDER BY tick_id DESC, attempt DESC LIMIT 1",
            params![before_tick_id],
            |row| row.get(0),
        )
        .context("get_last_renhun_narrative query 失败")
    }

    /// 按 tick_id 获取所有 attempt 的记录
    pub async fn get_by_tick(&self, tick_id: i64) -> anyhow::Result<Vec<SoulCycleRecord>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("soul_cycle_record lock poisoned: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, tick_id, attempt, renhun_narrative, renhun_thought_log,
                    tianhun_result, tianhun_layer1_result, tianhun_layer2_result,
                    tianhun_layer3_result, tianhun_reason,
                    final_intent_id, final_action_type, final_action_data, final_pipeline_json,
                    route_type, world_time, earth_tool_calls, model_id,
                    tianhun_layers, server_execution_results, created_at
             FROM soul_cycle_record WHERE tick_id = ?1 ORDER BY attempt ASC",
            )
            .context("get_by_tick prepare 失败")?;
        let rows = stmt
            .query_map(params![tick_id], |row| Ok(Self::row_to_record(row)))
            .context("get_by_tick query_map 失败")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("get_by_tick collect rows 失败")
    }

    /// 按 tick 分组的分页查询（返回去重 tick_id 列表）
    pub async fn get_tick_ids_page(
        &self,
        page: u32,
        limit: u32,
    ) -> anyhow::Result<(Vec<i64>, u32)> {
        let page = page.max(1);
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("soul_cycle_record lock poisoned: {e}"))?;
        let total: u32 = conn
            .query_row(
                "SELECT COUNT(DISTINCT tick_id) FROM soul_cycle_record",
                [],
                |row| row.get(0),
            )
            .context("get_tick_ids_page COUNT 失败")?;

        // page=0 的 u32 下溢与极大页码的乘法溢出均按饱和语义处理，
        // 越界页自然得到空结果
        let offset = (page.max(1) as i64 - 1).saturating_mul(limit as i64);
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT tick_id FROM soul_cycle_record ORDER BY tick_id DESC LIMIT ?1 OFFSET ?2",
            )
            .context("get_tick_ids_page prepare 失败")?;

        let tick_ids: Vec<i64> = stmt
            .query_map(params![limit, offset], |row| row.get(0))
            .context("get_tick_ids_page query_map 失败")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("get_tick_ids_page collect rows 失败")?;

        Ok((tick_ids, total))
    }

    /// 批量获取多个 tick 的三魂循环记录（分批查询消除 N+1 与 SQLite IN 变量数限制）
    ///
    /// tick_ids 超过单批上限时自动分批查询并合并，结果按 tick_id 降序、attempt 升序排列
    ///（与单批 SQL 的 ORDER BY tick_id DESC, attempt ASC 语义一致）。
    pub async fn get_by_ticks(&self, tick_ids: &[i64]) -> anyhow::Result<Vec<SoulCycleRecord>> {
        if tick_ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("soul_cycle_record lock poisoned: {e}"))?;

        let mut all: Vec<SoulCycleRecord> = Vec::new();
        for batch in tick_ids.chunks(TICK_BATCH_SIZE) {
            let sql = format!(
                "SELECT id, tick_id, attempt, renhun_narrative, renhun_thought_log,
                        tianhun_result, tianhun_layer1_result, tianhun_layer2_result,
                        tianhun_layer3_result, tianhun_reason,
                        final_intent_id, final_action_type, final_action_data, final_pipeline_json,
                        route_type, world_time, earth_tool_calls, model_id,
                        tianhun_layers, server_execution_results, created_at
                 FROM soul_cycle_record WHERE tick_id IN ({}) ORDER BY tick_id DESC, attempt ASC",
                build_in_placeholders(batch.len())
            );

            let mut stmt = conn.prepare(&sql).context("get_by_ticks prepare 失败")?;

            let params: Vec<&dyn rusqlite::ToSql> =
                batch.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let rows = stmt
                .query_map(params.as_slice(), |row| Ok(Self::row_to_record(row)))
                .context("get_by_ticks query_map 失败")?;
            all.extend(
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .context("get_by_ticks collect rows 失败")?,
            );
        }

        all.sort_by(|a, b| b.tick_id.cmp(&a.tick_id).then(a.attempt.cmp(&b.attempt)));
        Ok(all)
    }

    /// 批量获取多个 tick 的即时意图记录（分批查询，超过单批上限自动分批并合并）
    ///
    /// 结果按 id 升序排列（与单批 SQL 的 ORDER BY id ASC 语义一致）。
    pub async fn get_immediate_by_ticks(
        &self,
        tick_ids: &[i64],
    ) -> anyhow::Result<Vec<ImmediateIntentRecord>> {
        if tick_ids.is_empty() {
            return Ok(vec![]);
        }
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("soul_cycle_record lock poisoned: {e}"))?;

        let mut all: Vec<ImmediateIntentRecord> = Vec::new();
        for batch in tick_ids.chunks(TICK_BATCH_SIZE) {
            let sql = format!(
                "SELECT id, tick_id, intent_id, source_narrative, route_type,
                        action_type, action_data, speech_content, send_status, send_error, created_at
                 FROM immediate_intent_record WHERE tick_id IN ({}) ORDER BY id ASC",
                build_in_placeholders(batch.len())
            );

            let mut stmt = conn
                .prepare(&sql)
                .context("get_immediate_by_ticks prepare 失败")?;

            let params: Vec<&dyn rusqlite::ToSql> =
                batch.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let rows = stmt
                .query_map(params.as_slice(), |row| {
                    Ok(Self::row_to_immediate_record(row))
                })
                .context("get_immediate_by_ticks query_map 失败")?;
            all.extend(
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .context("get_immediate_by_ticks collect rows 失败")?,
            );
        }

        all.sort_by_key(|r| r.id);
        Ok(all)
    }

    /// 获取即时意图记录
    pub async fn get_immediate_by_tick(
        &self,
        tick_id: i64,
    ) -> anyhow::Result<Vec<ImmediateIntentRecord>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("soul_cycle_record lock poisoned: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, tick_id, intent_id, source_narrative, route_type,
                    action_type, action_data, speech_content, send_status, send_error, created_at
             FROM immediate_intent_record WHERE tick_id = ?1 ORDER BY id ASC",
            )
            .context("get_immediate_by_tick prepare 失败")?;
        let rows = stmt
            .query_map(params![tick_id], |row| {
                Ok(Self::row_to_immediate_record(row))
            })
            .context("get_immediate_by_tick query_map 失败")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("get_immediate_by_tick collect rows 失败")
    }

    fn row_to_record(row: &rusqlite::Row<'_>) -> SoulCycleRecord {
        let created_at_str: String = row.get(20).unwrap_or_default();
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
            final_intent_id: row.get(10).ok(),
            final_action_type: row.get(11).ok(),
            final_action_data: row.get(12).ok(),
            final_pipeline_json: row.get(13).ok(),
            route_type: row.get(14).unwrap_or_else(|_| "main".to_string()),
            world_time: row.get(15).ok(),
            earth_tool_calls: row.get(16).ok(),
            model_id: row.get(17).ok(),
            tianhun_layers: row.get(18).ok(),
            server_execution_results: row.get(19).ok(),
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
#[path = "soul_cycle_recorder_tests.rs"]
mod tests;
