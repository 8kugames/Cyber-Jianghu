// 记忆 API Handlers
// ============================================================================

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tracing::error;

use super::HttpApiState;
use super::service::{MemoryService, memories_to_json_response};

/// 获取近期记忆
pub(crate) async fn get_recent_memory_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let guard = state.memory_manager.read().await;
    let mm = match guard.as_ref() {
        Some(mm) => mm,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Memory manager not initialized",
            )
                .into_response();
        }
    };

    let mut mgr = mm.write().await;
    let service = MemoryService::new(&mut mgr);
    let memories = service.get_recent();

    Json(memories_to_json_response(&memories)).into_response()
}

const DEFAULT_PAGE_SIZE: usize = 20;
const MAX_PAGE_SIZE: usize = 100;

/// 获取每日摘要记忆
pub(crate) async fn get_daily_summaries_handler(
    State(state): State<HttpApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let guard = state.memory_manager.read().await;
    let mm = match guard.as_ref() {
        Some(mm) => mm,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Memory manager not initialized",
            )
                .into_response();
        }
    };

    let page: usize = params
        .get("page")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .max(1);
    let limit: usize = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .min(MAX_PAGE_SIZE);
    let offset = (page - 1) * limit;

    let mut mgr = mm.write().await;
    let service = MemoryService::new(&mut mgr);

    match service.get_daily_summaries(offset, limit) {
        Ok((memories, has_more)) => {
            let results: Vec<serde_json::Value> = memories
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "tick_id": m.tick_id,
                        "content": m.content,
                        "importance": m.importance_score,
                        "created_at": m.created_at.to_rfc3339(),
                    })
                })
                .collect();
            Json(serde_json::json!({
                "summaries": results,
                "count": results.len(),
                "has_more": has_more,
                "page": page,
                "limit": limit,
            }))
            .into_response()
        }
        Err(e) => {
            error!("[http] Failed to get daily summaries: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get daily summaries: {}", e),
            )
                .into_response()
        }
    }
}

/// 搜索记忆
pub(crate) async fn search_memory_handler(
    State(state): State<HttpApiState>,
    Json(request): Json<super::dto::MemorySearchRequest>,
) -> impl IntoResponse {
    let guard = state.memory_manager.read().await;
    let mm = match guard.as_ref() {
        Some(mm) => mm,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Memory manager not initialized",
            )
                .into_response();
        }
    };

    let mut mgr = mm.write().await;
    let mut service = MemoryService::new(&mut mgr);
    let limit = request.limit.unwrap_or(10);

    match service.search(&request.query, limit).await {
        Ok(memories) => Json(memories_to_json_response(&memories)).into_response(),
        Err(e) => {
            error!("[http] Failed to search memory: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Search failed: {}", e),
            )
                .into_response()
        }
    }
}

/// 存储记忆
pub(crate) async fn store_memory_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<super::dto::MemoryStoreRequest>,
) -> impl IntoResponse {
    let guard = state.memory_manager.read().await;
    let mm = match guard.as_ref() {
        Some(mm) => mm,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Memory manager not initialized",
            )
                .into_response();
        }
    };

    let tick_id = state
        .current_state
        .read()
        .await
        .as_ref()
        .map(|s| s.tick_id)
        .unwrap_or(0);
    let agent_id = *state.agent_id.read().await;
    let mut mgr = mm.write().await;
    let mut service = MemoryService::new(&mut mgr);

    match service
        .store(agent_id, tick_id, req.content, req.importance)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({"success": true, "message": "Memory stored"})),
        )
            .into_response(),
        Err(e) => {
            error!("[http] Failed to store memory: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to store memory: {}", e),
            )
                .into_response()
        }
    }
}

// ============================================================================
