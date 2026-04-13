// ============================================================================
// 群像传记 API Handler
// ============================================================================
//
// GET /api/dashboard/chronicles        - 列表
// GET /api/dashboard/chronicles/{id}  - 详情
// POST /api/dashboard/chronicles/generate - 手动生成
// ============================================================================

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use crate::chronicle;
use crate::state::AppState;

use crate::chronicle::ChronicleMeta;

/// 查询参数
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub page: Option<i32>,
    pub limit: Option<i32>,
}

/// 列表响应
#[derive(Debug, serde::Serialize)]
pub struct ListResponse {
    pub chronicles: Vec<ChronicleMeta>,
    pub total: i64,
    pub page: i32,
    pub limit: i32,
}

/// 获取群像传记列表
/// GET /api/dashboard/chronicles
pub async fn list_chronicles(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListQuery>,
) -> Result<Json<ListResponse>, axum::http::StatusCode> {
    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * limit;

    let chronicles = crate::chronicle::storage::list_chronicles(&state.db_pool, limit, offset)
        .await
        .map_err(|e| {
            tracing::error!("查询 chronicles 列表失败: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let total = crate::chronicle::storage::count_chronicles(&state.db_pool)
        .await
        .map_err(|e| {
            tracing::error!("查询 chronicle 总数失败: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(ListResponse {
        chronicles,
        total,
        page,
        limit,
    }))
}

/// 路径参数
#[derive(Debug, Deserialize)]
pub struct IdPath {
    pub id: String,
}

/// 获取群像传记详情
/// GET /api/dashboard/chronicles/{id}
pub async fn get_chronicle(
    State(state): State<Arc<AppState>>,
    Path(params): Path<IdPath>,
) -> Result<Json<chronicle::Chronicle>, axum::http::StatusCode> {
    match crate::chronicle::storage::get_chronicle(&state.db_pool, &params.id).await {
        Ok(Some(chronicle)) => Ok(Json(chronicle)),
        Ok(None) => Err(axum::http::StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!("获取 chronicle 详情失败: {}", e);
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// 手动生成请求
#[derive(Debug, Deserialize)]
pub struct GenerateRequest {
    pub period_start: Option<i64>,
    pub period_end: Option<i64>,
}

/// 手动生成群像传记
/// POST /api/dashboard/chronicles/generate
pub async fn generate_chronicle(
    State(state): State<Arc<AppState>>,
    Json(params): Json<GenerateRequest>,
) -> Result<Json<chronicle::Chronicle>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let current_tick = crate::db::get_current_world_tick_id(&state.db_pool)
        .await
        .unwrap_or(0);

    let period_end = params.period_end.unwrap_or(current_tick);
    let period_start = params
        .period_start
        .unwrap_or_else(|| chronicle::calculate_period_start(period_end));

    match chronicle::generate_and_store(period_start, period_end, &state.db_pool).await {
        Ok(chronicle) => Ok(Json(chronicle)),
        Err(e) => {
            tracing::error!("生成 chronicle 失败: {}", e);
            Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ))
        }
    }
}

/// 获取 LLM Token 统计
///
/// GET /api/dashboard/chronicles/llm-stats
pub async fn get_llm_stats() -> Json<serde_json::Value> {
    let (input_tokens, output_tokens, request_count, error_count) =
        chronicle::generator::get_llm_stats();

    Json(serde_json::json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_requests": request_count,
        "error_count": error_count,
    }))
}

/// 获取进行中的异步生成任务
///
/// GET /api/dashboard/chronicles/pending
pub async fn get_pending_generations() -> Json<serde_json::Value> {
    let tracker = chronicle::get_generation_tracker();
    let tasks = tracker.get_pending_tasks().await;

    // 格式化为友好的响应
    let task_summaries: Vec<serde_json::Value> = tasks
        .iter()
        .map(|t| {
            let status_str = match &t.status {
                chronicle::GenerationStatus::Pending => "pending",
                chronicle::GenerationStatus::Generating => "generating",
                chronicle::GenerationStatus::Completed => "completed",
                chronicle::GenerationStatus::Failed(e) => {
                    return serde_json::json!({
                        "chronicle_id": t.chronicle_id,
                        "status": "failed",
                        "error": e,
                        "started_at": t.started_at,
                        "completed_at": t.completed_at,
                    });
                }
            };

            let supplement_str = match &t.supplement_status {
                chronicle::GenerationStatus::Pending => "pending",
                chronicle::GenerationStatus::Generating => "generating",
                chronicle::GenerationStatus::Completed => "completed",
                chronicle::GenerationStatus::Failed(e) => {
                    return serde_json::json!({
                        "chronicle_id": t.chronicle_id,
                        "status": status_str,
                        "supplement_status": "failed",
                        "supplement_error": e,
                        "primary_version": t.primary_version,
                        "started_at": t.started_at,
                        "completed_at": t.completed_at,
                    });
                }
            };

            serde_json::json!({
                "chronicle_id": t.chronicle_id,
                "status": status_str,
                "primary_version": t.primary_version,
                "supplement_status": supplement_str,
                "started_at": t.started_at,
                "completed_at": t.completed_at,
            })
        })
        .collect();

    Json(serde_json::json!({
        "pending_count": task_summaries.len(),
        "tasks": task_summaries,
    }))
}
