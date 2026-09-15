use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::db::DbPool;
use crate::handlers::pagination::offset_of;
use crate::state::AppState;

use super::card::{
    build_execution_results, card_success, inject_execution_results, resolve_experience_model,
    row_success,
};

/// 动作发生时位置的回退子查询片段
///
/// 取该 agent 不晚于该动作 tick 的最近一条 agent_states.node_id。
/// 计数、分页取键、取数三处必须共用同一片段，否则筛选口径会漂移：
/// 位置筛选打在行上，而分页单位是 (agent_id, tick_id) 卡片。
const EXPERIENCE_LOCATION_LATERAL: &str = r#"
        LEFT JOIN LATERAL (
            SELECT st2.node_id
            FROM agent_states st2
            WHERE st2.agent_id = a.agent_id AND st2.tick_id <= a.tick_id
            ORDER BY st2.tick_id DESC
            LIMIT 1
        ) loc ON true
        "#;

/// 经历流水的筛选谓词片段（位置筛选由 LATERAL 别名 loc 提供）
///
/// 占位符编号必须与调用点 `.bind()` 链的顺序逐位对应：
/// $1 agent_id(uuid) / $2 location(text) / $3 action_type(text) /
/// $4 from_tick(bigint) / $5 to_tick(bigint) / $6 result(text)。
///
/// 这不是风格问题：sqlx 会把绑定值的 Rust 类型作为参数类型发给 Postgres
/// 用于 Parse 阶段，编号与绑定值错位会让语句在解析期直接失败
/// （例如 `a.result = $1` 收到 uuid 时报 character varying = uuid）。
/// 新增或调整任一筛选条件时，必须同时改这里和两处 `.bind()`。
///
/// $6 的判断单位是**卡片**而非行，与前端按 tick 聚合出的成败徽章同一口径
/// （徽章 = 该 tick 的所有 pipe_seq 行全部 success 才是成功，见
/// `build_execution_results`）。若按行判断，同一张卡片会既出现在
/// "仅成功"视图里、又顶着"失败"徽章，筛选与徽章自相矛盾。
/// 只认 all / success / failed 三个取值，其余取值不匹配任何行。
const EXPERIENCE_ROW_FILTERS: &str = r#"
        WHERE ($6::text = 'all'
               OR ($6::text = 'success' AND NOT EXISTS (
                     SELECT 1 FROM agent_action_logs f
                     WHERE f.agent_id = a.agent_id AND f.tick_id = a.tick_id
                       AND f.result IS DISTINCT FROM 'success'))
               OR ($6::text = 'failed' AND EXISTS (
                     SELECT 1 FROM agent_action_logs f
                     WHERE f.agent_id = a.agent_id AND f.tick_id = a.tick_id
                       AND f.result IS DISTINCT FROM 'success')))
          AND ($1::uuid IS NULL OR a.agent_id = $1)
          AND ($3::text IS NULL OR a.action_type = $3)
          AND ($4::bigint IS NULL OR a.tick_id >= $4)
          AND ($5::bigint IS NULL OR a.tick_id <= $5)
          AND ($2::text IS NULL OR loc.node_id = $2)
        "#;

/// 经历流水的筛选条件（对应页面筛选项）
///
/// 字段顺序不代表绑定顺序：绑定发生在下面两个查询函数内部，
/// 与 `EXPERIENCE_ROW_FILTERS` 的占位符逐位对应。
pub struct ExperienceStreamFilters<'a> {
    /// "all" → 全部；"success" → 该 tick 全部动作成功；"failed" → 该 tick 存在失败动作。
    /// 判定单位是卡片，与卡片徽章同口径；其余取值不匹配任何行。
    pub result: &'a str,
    pub agent_id: Option<Uuid>,
    pub location: Option<&'a str>,
    pub action_type: Option<&'a str>,
    pub from_tick: Option<i64>,
    pub to_tick: Option<i64>,
}

/// 为带 `EXPERIENCE_ROW_FILTERS` 的语句绑定筛选参数
///
/// 这是 $1..$6 顺序的唯一书写点：片段与绑定值分处两地，任何一处改动都必须
/// 同时改另一处，因此把绑定收敛成一个函数，避免 count / keys 各写一遍后
/// 静默错位（错位的后果不是报错而是 Postgres 在 Parse 期类型不匹配）。
fn bind_row_filters<'q, O>(
    query: sqlx::query::QueryAs<'q, sqlx::Postgres, O, sqlx::postgres::PgArguments>,
    f: &'q ExperienceStreamFilters<'q>,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, O, sqlx::postgres::PgArguments>
where
    O: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
{
    query
        .bind(f.agent_id)
        .bind(f.location)
        .bind(f.action_type)
        .bind(f.from_tick)
        .bind(f.to_tick)
        .bind(f.result)
}

/// 统计符合条件的 tick 卡片总数
///
/// 与 `fetch_experience_tick_keys` 共用 `EXPERIENCE_ROW_FILTERS` 与
/// `bind_row_filters`，两处口径必然一致。
pub async fn count_experience_ticks(
    pool: &DbPool,
    f: &ExperienceStreamFilters<'_>,
) -> Result<i64, sqlx::Error> {
    let sql = format!(
        "SELECT COUNT(*) FROM (SELECT 1 FROM agent_action_logs a \
         {EXPERIENCE_LOCATION_LATERAL} {EXPERIENCE_ROW_FILTERS} \
         GROUP BY a.agent_id, a.tick_id) g"
    );
    let (total,): (i64,) = bind_row_filters(sqlx::query_as(&sql), f)
        .fetch_one(pool)
        .await?;
    Ok(total)
}

/// 取本页 tick 卡片键（$7 limit / $8 offset）
///
/// ORDER BY 是分页稳定性的唯一来源：tick_id DESC 为主序，agent_id 作
/// tiebreaker，避免同一 tick 多角色时跨页错位。
pub async fn fetch_experience_tick_keys(
    pool: &DbPool,
    f: &ExperienceStreamFilters<'_>,
    limit: i64,
    offset: i64,
) -> Result<Vec<(Uuid, i64)>, sqlx::Error> {
    let sql = format!(
        "SELECT a.agent_id, a.tick_id FROM agent_action_logs a \
         {EXPERIENCE_LOCATION_LATERAL} {EXPERIENCE_ROW_FILTERS} \
         GROUP BY a.agent_id, a.tick_id \
         ORDER BY a.tick_id DESC, a.agent_id \
         LIMIT $7 OFFSET $8"
    );
    bind_row_filters(sqlx::query_as(&sql), f)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}
// ============================================================================
// Experience Stream API (经历日志流水)
// ============================================================================

/// 经历日志流水查询参数
#[derive(Debug, Deserialize)]
pub struct ExperienceStreamQuery {
    pub page: Option<i32>,
    pub limit: Option<i32>,
    pub agent_id: Option<Uuid>,
    pub location: Option<String>,
    pub action_type: Option<String>,
    pub from_tick: Option<i64>,
    pub to_tick: Option<i64>,
    /// 结果过滤: "success" | "failed" | 空=全部
    pub result: Option<String>,
}

/// 经历日志流水条目
#[derive(Debug, Serialize)]
pub struct StreamEntry {
    pub tick_id: i64,
    pub agent_id: Uuid,
    pub device_id: Option<Uuid>,
    pub agent_name: String,
    pub location: Option<String>,
    pub action_type: String,
    pub action_type_display: Option<String>,
    pub action_data: serde_json::Value,
    pub result: Option<String>,
    pub result_message: Option<String>,
    pub thought_log: Option<String>,
    pub reflector_thought: Option<String>,
    pub narrative: Option<String>,
    pub soul_cycle_metadata: Option<serde_json::Value>,
    /// 模型 ID 权威归一值（同 `ExperienceEntry.model_id`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// 游戏日编号（0 表示无元数据）
    pub game_day: i64,
    /// 中文时间（由 `WorldTime::to_chinese()` 生成，无法解析时为 "-"），供前端直接渲染
    pub formatted_time: String,
    /// 卡片级成败：该 tick 的全部 pipe_seq 行都 success 才为 true。
    ///
    /// 与 `EXPERIENCE_ROW_FILTERS` 的 $6 判定同一口径，卡片徽章因此与筛选视图
    /// 取自同一判定（同快照下一致）。不能由前端从
    /// `soul_cycle_metadata.execution_results` 推导：无元数据的卡片
    /// （`inject_execution_results` 对 metadata 为 None 的卡片不产出
    /// execution_results，生产库约四千余张，其中约百张含失败行）根本没有该字段，
    /// 前端只能退回主行 result，于是出现"在仅失败视图里顶着成功徽章"——
    /// 实测这一个子集在数十张量级（2026-09-15 活库采样）。
    pub card_success: bool,
    /// 该 tick 除首行（主行动）外 result 非 success 的动作摘要。
    ///
    /// 无元数据卡片没有 `soul_cycle_metadata.execution_results` 可渲染，
    /// 降级路径此前完全看不到这些失败行，只显示"主行成功"——与卡片级
    /// 失败徽章自相矛盾。主行自身成败已由 `result`/`result_message` 表达，
    /// 故此处排除首行避免重复。有元数据卡片走 execution_results 渲染，
    /// 前端不读此字段。
    pub other_failed_actions: Vec<FailedAction>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 降级渲染所需的失败动作摘要（见 `StreamEntry::other_failed_actions`）
#[derive(Debug, Serialize)]
pub struct FailedAction {
    pub pipe_seq: i32,
    pub action_type: String,
    pub action_type_display: Option<String>,
    pub result_message: Option<String>,
}

/// 经历日志流水响应
#[derive(Debug, Serialize)]
pub struct ExperienceStreamResponse {
    pub entries: Vec<StreamEntry>,
    pub total: i64,
    pub page: i32,
    pub limit: i32,
}

/// GET /api/dashboard/experiences
///
/// 返回 agent 动作日志（全局视图），用于经历日志流水。
/// 默认只返回整体成功的卡片，传 result=all 查看全部。
///
/// 分页单位是 tick 卡片而非行：页内每个 (agent_id, tick_id) 聚合成一条
/// StreamEntry，同一 tick 的多条 pipe_seq 行归并进 execution_results。
/// agent / location / action_type 按行判定（命中该 tick 即整张卡片入选），
/// result 按卡片判定（与卡片徽章同口径）；入选后该 tick 的全部 pipe_seq 行
/// 都会被取回，使三魂元数据所在的主行不会因筛选而缺席，卡片内容保持完整。
pub async fn get_experiences(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExperienceStreamQuery>,
) -> Result<Json<ExperienceStreamResponse>, StatusCode> {
    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    // page 由查询参数直出：page = i32::MAX 时 page - 1 的 i32 乘法先溢出，
    // debug 构建 panic 中断请求任务（连接被丢弃而非 500），release 构建回绕成负偏移。
    // 改按 i64 计算并饱和到 i32：越界页自然得到空集。
    let offset = offset_of(page, limit);

    // 构建过滤条件
    let filters = ExperienceStreamFilters {
        // result 过滤: None/空 → 只看成功, "failed" → 只看失败, "all" → 全部
        result: params.result.as_deref().unwrap_or("success"),
        agent_id: params.agent_id,
        location: params.location.as_deref(),
        action_type: params.action_type.as_deref(),
        from_tick: params.from_tick,
        to_tick: params.to_tick,
    };

    let total: i64 = count_experience_ticks(&state.db_pool, &filters)
        .await
        .map_err(|e| {
            // 不降级为 total=0：计数失败曾把"SQL 绑定错位导致端点 500"伪装成
            // "共 0 个 tick"的正常响应，排查成本远高于直接报错
            tracing::error!("Failed to count experience ticks: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let keys: Vec<(Uuid, i64)> =
        fetch_experience_tick_keys(&state.db_pool, &filters, limit as i64, offset)
            .await
            .map_err(|e| {
                tracing::error!("Failed to fetch experience tick keys: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

    if keys.is_empty() {
        return Ok(Json(ExperienceStreamResponse {
            entries: Vec::new(),
            total,
            page,
            limit,
        }));
    }

    let key_agent_ids: Vec<Uuid> = keys.iter().map(|(agent_id, _)| *agent_id).collect();
    let key_tick_ids: Vec<i64> = keys.iter().map(|(_, tick_id)| *tick_id).collect();

    // 取回入选 tick 的全部 pipe_seq 行（不再重复施加行级筛选，见函数文档）
    let rows_sql = format!(
        "SELECT a.tick_id, a.agent_id, ag.device_id, ag.name as agent_name, loc.node_id as location, \
                a.pipe_seq, a.action_type, a.action_type_display, a.action_data, \
                a.result, a.result_message, a.thought_log, a.reflector_thought, \
                a.narrative, a.soul_cycle_metadata, a.created_at, \
                a.soul_cycle_metadata->'cycles'->0->>'model_id' AS per_row_model_id, \
                ag.model_id AS agent_model_id \
         FROM agent_action_logs a \
         JOIN agents ag ON a.agent_id = ag.agent_id \
         JOIN unnest($1::uuid[], $2::bigint[]) AS k(agent_id, tick_id) \
           ON k.agent_id = a.agent_id AND k.tick_id = a.tick_id \
         {EXPERIENCE_LOCATION_LATERAL} \
         ORDER BY a.tick_id DESC, a.agent_id, a.pipe_seq ASC"
    );
    let rows = sqlx::query(&rows_sql)
        .bind(&key_agent_ids)
        .bind(&key_tick_ids)
        .fetch_all(&state.db_pool)
        .await
        .map_err(|e| {
            tracing::error!("获取经历日志流水失败: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // 按 (agent_id, tick_id) 分组，合并多 pipe_seq 行为 execution_results
    let mut grouped: HashMap<(Uuid, i64), Vec<sqlx::postgres::PgRow>> = HashMap::new();
    for row in rows {
        let key = (row.get::<Uuid, _>("agent_id"), row.get::<i64, _>("tick_id"));
        grouped.entry(key).or_default().push(row);
    }

    // 按 keys 顺序出卡片，保证响应顺序与分页顺序一致（HashMap 迭代序不可用）
    let mut entries: Vec<StreamEntry> = Vec::with_capacity(keys.len());
    for key in &keys {
        let Some(group) = grouped.remove(key) else {
            // 取键与取数分属两条自动提交语句，且取数额外 JOIN agents：
            // 行被并发清理时该 tick 会缺席，仅记日志不静默丢卡
            tracing::warn!(
                "经历日志卡片缺少对应动作行（agent_id={}, tick_id={}），本页少一张卡片",
                key.0,
                key.1
            );
            continue;
        };
        // 已由 SQL 按 pipe_seq 升序保证：首行是该 tick 管道首条动作（min(pipe_seq)，
        // 通常为 0），也是三魂元数据的所在行
        let primary = group.first().expect("group must have at least one row");
        let metadata: Option<serde_json::Value> = primary.get("soul_cycle_metadata");
        let world_time_json: Option<String> = metadata
            .as_ref()
            .and_then(|m| m.get("world_time"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let execution_results = build_execution_results(&group);
        let enriched_metadata = inject_execution_results(metadata, &execution_results);
        let other_failed_actions = group[1..]
            .iter()
            .filter(|row| !row_success(row.get::<Option<String>, _>("result").as_deref()))
            .map(|row| FailedAction {
                pipe_seq: row.get("pipe_seq"),
                action_type: row.get("action_type"),
                action_type_display: row.get("action_type_display"),
                result_message: row.get("result_message"),
            })
            .collect();

        entries.push(StreamEntry {
            tick_id: primary.get("tick_id"),
            agent_id: primary.get("agent_id"),
            device_id: primary.get("device_id"),
            agent_name: primary.get("agent_name"),
            location: primary.get("location"),
            action_type: primary.get("action_type"),
            action_type_display: primary.get("action_type_display"),
            action_data: primary
                .get::<Option<serde_json::Value>, _>("action_data")
                .unwrap_or(serde_json::Value::Null),
            result: primary.get("result"),
            result_message: primary.get("result_message"),
            thought_log: primary.get("thought_log"),
            reflector_thought: primary.get("reflector_thought"),
            narrative: primary.get("narrative"),
            soul_cycle_metadata: enriched_metadata,
            model_id: resolve_experience_model(&group, primary.get("agent_model_id")),
            game_day: crate::time_utils::world_time_json_to_game_day(world_time_json.as_deref()),
            formatted_time: crate::time_utils::world_time_json_to_chinese(
                world_time_json.as_deref(),
            ),
            card_success: card_success(&group),
            other_failed_actions,
            created_at: primary.get("created_at"),
        });
    }

    Ok(Json(ExperienceStreamResponse {
        entries,
        total,
        page,
        limit,
    }))
}
