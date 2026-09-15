use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::handlers::pagination::offset_of;
use crate::state::AppState;

use super::card::{build_execution_results, inject_execution_results, resolve_experience_model};

/// 经历日志条目
#[derive(Debug, serde::Serialize)]
pub struct ExperienceEntry {
    pub tick_id: i64,
    /// 动作原始类型（如 idle, speak）
    pub action_type: String,
    /// 动作中文描述（如 "静修"、"交谈"）
    pub action_type_display: Option<String>,
    pub action_data: serde_json::Value,
    /// 执行结果（success/failed）
    pub result: Option<String>,
    /// 执行结果详细描述
    pub result_message: Option<String>,
    /// ActorSoul 思考日志
    pub thought_log: Option<String>,
    /// ReflectorSoul 审查理由
    pub reflector_thought: Option<String>,
    /// 叙事化经历描述
    pub narrative: Option<String>,
    /// 三魂循环元数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soul_cycle_metadata: Option<serde_json::Value>,
    /// 模型 ID 权威归一值（per-attempt first cycle model_id → agents.model_id 兜底），供前端直接渲染
    /// 原方案是 history.js 从 soul_cycle_metadata.cycles 数组扫首个非空，脆弱
    /// 且 soul_cycle_metadata 为 null 时无从取值。此处统一由 server 侧归一，
    /// 前端不再做 JSONB 字符串提取；为空时前端渲染「模型未上报」。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// 游戏日编号（从 soul_cycle_metadata.world_time 解析，无元数据时为 0）
    pub game_day: i64,
    /// 中文时间（由 `WorldTime::to_chinese()` 生成，无法解析时为 "-"），供前端直接渲染
    pub formatted_time: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 经历日志响应
#[derive(Debug, serde::Serialize)]
pub struct ExperiencesResponse {
    pub experiences: Vec<ExperienceEntry>,
    pub total: i64,
    pub page: i32,
    pub limit: i32,
}

/// 获取 Agent 经历日志
///
/// 支持两种认证方式：
/// 1. Admin token (Bearer auth): 查看任意角色的经历日志
/// 2. Device auth (query params): 设备只能查看自己归属角色的经历日志
///
/// GET /api/dashboard/agent/{id}/experiences?page=1&limit=20&device_id=xxx&auth_token=yyy
pub async fn get_agent_experiences(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ExperiencesResponse>, StatusCode> {
    // 设备认证：如果提供了 device_id 和 auth_token，使用设备归属校验
    if let (Some(device_id_str), Some(auth_token)) =
        (params.get("device_id"), params.get("auth_token"))
        && let Ok(device_id) = Uuid::parse_str(device_id_str)
    {
        match crate::db::verify_device_token(&state.db_pool, device_id, auth_token).await {
            Ok(true) => {
                // 验证通过，检查设备是否归属该 agent
                let owner_device_id: Option<Uuid> =
                    sqlx::query_scalar("SELECT device_id FROM agents WHERE agent_id = $1")
                        .bind(agent_id)
                        .fetch_optional(&state.db_pool)
                        .await
                        .map_err(|e| {
                            // 不吞错降级为 None：那会把数据库故障伪装成"设备不归属"
                            // 而返回 403，把基础设施问题伪装成权限问题
                            tracing::error!("Failed to verify agent owner device: {}", e);
                            StatusCode::INTERNAL_SERVER_ERROR
                        })?;

                if owner_device_id != Some(device_id) {
                    tracing::warn!(
                        "Device {} attempted to access agent {} experiences without ownership",
                        device_id,
                        agent_id
                    );
                    return Err(StatusCode::FORBIDDEN);
                }
            }
            Ok(false) => return Err(StatusCode::UNAUTHORIZED),
            Err(e) => {
                tracing::warn!("Device token verify error: {}", e);
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    // page/limit 由查询参数直出：不钳位时 `?limit=-1` 会让 PostgreSQL 报错返回 500，
    // 越界 page 会让偏移量溢出（详见 `crate::handlers::pagination`）。与流水端点同规范。
    let page = params.get("page").and_then(|s| s.parse().ok()).unwrap_or(1);
    let limit: i32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20)
        .clamp(1, 100);
    let page = page.max(1);
    let offset = offset_of(page, limit);

    // 获取经历日志总 tick 数（按 tick_id 分组计数）
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT tick_id) FROM agent_action_logs WHERE agent_id = $1",
    )
    .bind(agent_id)
    .fetch_one(&state.db_pool)
    .await
    .map_err(|e| {
        // 与全局流水端点同一处理：不降级为 total=0，否则解码/语句失败会被
        // 伪装成"该角色共 0 个 tick"的正常响应（COUNT 返回 BIGINT，须按 i64 解码）
        tracing::error!("Failed to count agent experience ticks: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // 先获取分页的 tick_id 列表，再批量拉取全部 pipe_seq 行
    let tick_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT tick_id
         FROM agent_action_logs
         WHERE agent_id = $1
         ORDER BY tick_id DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(agent_id)
    .bind(limit as i64)
    .bind(offset)
    .fetch_all(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch experience tick_ids: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if tick_ids.is_empty() {
        return Ok(Json(ExperiencesResponse {
            experiences: Vec::new(),
            total,
            page,
            limit,
        }));
    }

    // 构建 IN 子句参数（sqlx 不支持变长 IN，用 = ANY 替代）
    // model_id 由 resolve_experience_model 归一（per-row → agents.model_id），
    // 与全局流水端点共用同一口径。
    let rows = sqlx::query(
        "SELECT a.tick_id, a.action_type, a.action_type_display, a.action_data, a.result, a.result_message,
                a.thought_log, a.reflector_thought, a.narrative, a.soul_cycle_metadata, a.pipe_seq, a.created_at,
                a.soul_cycle_metadata->'cycles'->0->>'model_id' AS per_row_model_id,
                ag.model_id AS agent_model_id
         FROM agent_action_logs a
         JOIN agents ag ON a.agent_id = ag.agent_id
         WHERE a.agent_id = $1 AND a.tick_id = ANY($2)
         ORDER BY a.tick_id DESC, a.pipe_seq ASC",
    )
    .bind(agent_id)
    .bind(&tick_ids)
    .fetch_all(&state.db_pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch experiences: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // 按 tick_id 分组，合并多 pipe_seq 行为 execution_results
    use std::collections::HashMap;
    let mut grouped: HashMap<i64, Vec<sqlx::postgres::PgRow>> = HashMap::new();
    for row in rows {
        let tid: i64 = row.get("tick_id");
        grouped.entry(tid).or_default().push(row);
    }

    let experiences: Vec<ExperienceEntry> = grouped
        .into_values()
        .map(|group| {
            // 按 pipe_seq 升序排列（已由 SQL 保证）
            // 首行是管道首条动作（通常 pipe_seq=0；生产库存在 97 张卡片首行从 1
            // 起，历史数据缺 0 号行），也是三魂元数据的所在行
            let primary = group.first().expect("group must have at least one row");
            let metadata: Option<serde_json::Value> = primary.get("soul_cycle_metadata");
            let world_time_json: Option<String> = metadata
                .as_ref()
                .and_then(|m| m.get("world_time"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let execution_results = build_execution_results(&group);
            let enriched_metadata = inject_execution_results(metadata, &execution_results);

            ExperienceEntry {
                tick_id: primary.get("tick_id"),
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
                game_day: crate::time_utils::world_time_json_to_game_day(
                    world_time_json.as_deref(),
                ),
                formatted_time: crate::time_utils::world_time_json_to_chinese(
                    world_time_json.as_deref(),
                ),
                created_at: primary.get("created_at"),
            }
        })
        .collect();

    Ok(Json(ExperiencesResponse {
        experiences,
        total,
        page,
        limit,
    }))
}
