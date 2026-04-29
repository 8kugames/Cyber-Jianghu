// ============================================================================
// Agent 每日摘要 API Handler
// ============================================================================
//
// GET /api/dashboard/agent-daily-summaries        - 列表
// GET /api/dashboard/agent-daily-summaries/{agent_id} - 单个Agent的摘要列表
// ============================================================================

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use crate::db;
use crate::state::AppState;

/// 查询参数
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub agent_id: Option<String>,
    pub game_day: Option<i64>,
    pub page: Option<i32>,
    pub limit: Option<i32>,
}

/// 列表响应
#[derive(Debug, serde::Serialize)]
pub struct ListResponse {
    pub summaries: Vec<db::AgentDailySummary>,
    pub total: i64,
    pub page: i32,
    pub limit: i32,
}

/// 获取每日摘要列表
/// GET /api/dashboard/agent-daily-summaries
pub async fn list_summaries(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListQuery>,
) -> Result<Json<ListResponse>, axum::http::StatusCode> {
    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * limit;

    let agent_id = params
        .agent_id
        .as_ref()
        .and_then(|s| Uuid::parse_str(s).ok());
    let game_day = params.game_day;

    let summaries =
        db::list_agent_daily_summaries(&state.db_pool, agent_id, game_day, Some(limit as i64), Some(offset as i64))
            .await
            .map_err(|e| {
                tracing::error!("查询 agent_daily_summaries 列表失败: {}", e);
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            })?;

    let total = db::count_agent_daily_summaries(&state.db_pool, agent_id, game_day)
        .await
        .map_err(|e| {
            tracing::error!("统计 agent_daily_summaries 总数失败: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(ListResponse {
        summaries,
        total,
        page,
        limit,
    }))
}

/// 路径参数
#[derive(Debug, Deserialize)]
pub struct AgentIdPath {
    pub agent_id: String,
}

/// 获取指定 Agent 的每日摘要列表
/// GET /api/dashboard/agent-daily-summaries/{agent_id}
pub async fn get_by_agent(
    State(state): State<Arc<AppState>>,
    Path(params): Path<AgentIdPath>,
    Query(params_q): Query<ListQuery>,
) -> Result<Json<ListResponse>, axum::http::StatusCode> {
    let agent_id = Uuid::parse_str(&params.agent_id).map_err(|_| {
        axum::http::StatusCode::BAD_REQUEST
    })?;

    let page = params_q.page.unwrap_or(1).max(1);
    let limit = params_q.limit.unwrap_or(20).clamp(1, 100) as i64;
    let offset = ((page - 1) as i64) * limit;

    let summaries =
        db::get_agent_daily_summaries_by_agent(&state.db_pool, agent_id, Some(limit), Some(offset))
            .await
            .map_err(|e| {
                tracing::error!("查询 Agent {} daily_summaries 失败: {}", agent_id, e);
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            })?;

    let total = db::count_agent_daily_summaries(&state.db_pool, Some(agent_id), None)
        .await
        .map_err(|e| {
            tracing::error!("统计 Agent {} daily_summaries 总数失败: {}", agent_id, e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(ListResponse {
        summaries,
        total,
        page,
        limit: limit as i32,
    }))
}