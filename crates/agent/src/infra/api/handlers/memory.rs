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

const DEFAULT_PAGE_SIZE: usize = 20;
const MAX_PAGE_SIZE: usize = 100;
/// 非当前角色文本搜索的候选集大小
const CROSS_AGENT_SEARCH_CANDIDATES: usize = 200;

// ---------------------------------------------------------------------------
// 共享 helper：为指定角色临时打开 MemoryStore（只读）
// ---------------------------------------------------------------------------

/// 临时打开指定角色的 MemoryStore（只读）。
/// 返回 None 表示 DB 文件不存在或打开失败（已打 error 日志）。
fn open_agent_store(
    character_dir: &std::path::Path,
    agent_id: uuid::Uuid,
) -> Option<crate::component::memory::store::MemoryStore> {
    let data_dir = character_dir.join(agent_id.to_string()).join("data");
    let db_path = data_dir.join(format!("agent_{}.db", agent_id));
    if !db_path.exists() {
        return None;
    }
    match crate::component::memory::store::MemoryStore::new(agent_id, &data_dir) {
        Ok(store) => Some(store),
        Err(e) => {
            error!(
                "[http] Failed to open memory store for agent {}: {}",
                agent_id, e
            );
            None
        }
    }
}

/// ClientMemory → JSON 响应值
fn memory_to_json(m: &crate::component::memory::store::ClientMemory) -> serde_json::Value {
    serde_json::json!({
        "id": m.id,
        "tick_id": m.tick_id,
        "content": m.content,
        "importance": m.importance_score,
        "created_at": m.created_at.clone(),
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// 获取近期记忆
///
/// 支持可选 `agent_id` 查询参数：当指定非当前角色时，临时打开该角色的 DB 读取。
pub(crate) async fn get_recent_memory_handler(
    State(state): State<HttpApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let target_agent_id: Option<uuid::Uuid> = params
        .get("agent_id")
        .and_then(|s| uuid::Uuid::parse_str(s).ok());

    let current_agent_id = *state.agent_id.read().await;

    // 非当前角色 → 临时打开 DB 读取
    if let Some(target) = target_agent_id
        && target != current_agent_id
    {
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
        return read_memories_for_agent(&state, target, page, limit).await;
    }

    // 当前角色 → 使用内存中的 MemoryManager
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
    let memories = service.get_recent().await;
    Json(memories_to_json_response(&memories)).into_response()
}

/// 临时打开指定角色的记忆 DB 并返回近期记忆（支持分页）
async fn read_memories_for_agent(
    state: &HttpApiState,
    agent_id: uuid::Uuid,
    page: usize,
    limit: usize,
) -> axum::response::Response {
    let character_dir = state.character_dir.read().await.clone();
    let Some(store) = open_agent_store(&character_dir, agent_id) else {
        return Json(memories_to_json_response(&[])).into_response();
    };

    // 按时间排序分页（与前端"近期记忆"语义一致）
    let fetch = (page * limit).min(MAX_PAGE_SIZE);
    match store.get_recent_memories(fetch) {
        Ok(all) => {
            let offset = (page - 1) * limit;
            let page_slice: Vec<_> = all
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect();
            let has_more = page_slice.len() == limit;
            let results: Vec<serde_json::Value> = page_slice.iter().map(memory_to_json).collect();
            Json(serde_json::json!({
                "memories": results,
                "count": results.len(),
                "has_more": has_more,
            }))
            .into_response()
        }
        Err(e) => {
            error!(
                "[http] Failed to read memories for agent {}: {}",
                agent_id, e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read memories",
            )
                .into_response()
        }
    }
}

/// 获取每日摘要记忆
///
/// 支持可选 `agent_id` 查询参数：当指定非当前角色时，临时打开该角色的 DB 读取。
pub(crate) async fn get_daily_summaries_handler(
    State(state): State<HttpApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
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

    let target_agent_id: Option<uuid::Uuid> = params
        .get("agent_id")
        .and_then(|s| uuid::Uuid::parse_str(s).ok());
    let current_agent_id = *state.agent_id.read().await;

    // 非当前角色 → 临时打开 DB 读取
    if let Some(target) = target_agent_id
        && target != current_agent_id
    {
        return daily_summaries_for_agent(&state, target, offset, limit, page).await;
    }

    // 当前角色 → 使用内存中的 MemoryManager
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

/// 临时打开指定角色的记忆 DB 并返回每日摘要
async fn daily_summaries_for_agent(
    state: &HttpApiState,
    agent_id: uuid::Uuid,
    offset: usize,
    limit: usize,
    page: usize,
) -> axum::response::Response {
    let character_dir = state.character_dir.read().await.clone();
    let Some(store) = open_agent_store(&character_dir, agent_id) else {
        return Json(serde_json::json!({
            "summaries": [],
            "count": 0,
            "has_more": false,
            "page": page,
            "limit": limit,
        }))
        .into_response();
    };

    match store.get_memories_by_type("daily_summary", offset, limit + 1) {
        Ok(memories) => {
            let has_more = memories.len() > limit;
            let results: Vec<serde_json::Value> = memories
                .into_iter()
                .take(limit)
                .map(|m| memory_to_json(&m))
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
            error!(
                "[http] Failed to read daily summaries for agent {}: {}",
                agent_id, e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read daily summaries",
            )
                .into_response()
        }
    }
}

/// 搜索记忆
///
/// 支持可选 `agent_id` body 字段：当指定非当前角色时，临时打开该角色的 DB 做文本搜索。
pub(crate) async fn search_memory_handler(
    State(state): State<HttpApiState>,
    Json(request): Json<super::dto::MemorySearchRequest>,
) -> impl IntoResponse {
    let limit = request.limit.unwrap_or(10);
    let current_agent_id = *state.agent_id.read().await;

    // 非当前角色 → 临时打开 DB 做文本搜索（降级方案，无语义索引）
    if let Some(ref agent_id_str) = request.agent_id
        && let Ok(target) = uuid::Uuid::parse_str(agent_id_str)
        && target != current_agent_id
    {
        return search_memories_for_agent(&state, target, &request.query, limit).await;
    }

    // 当前角色 → 使用内存中的 MemoryManager（语义搜索）
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

/// 临时打开指定角色的记忆 DB 做文本搜索（非当前角色无法使用语义搜索）
async fn search_memories_for_agent(
    state: &HttpApiState,
    agent_id: uuid::Uuid,
    query: &str,
    limit: usize,
) -> axum::response::Response {
    let character_dir = state.character_dir.read().await.clone();
    let Some(store) = open_agent_store(&character_dir, agent_id) else {
        return Json(memories_to_json_response(&[])).into_response();
    };

    let all = match store.get_top_memories(CROSS_AGENT_SEARCH_CANDIDATES) {
        Ok(m) => m,
        Err(e) => {
            error!(
                "[http] Failed to read memories for search agent {}: {}",
                agent_id, e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read memories for search",
            )
                .into_response();
        }
    };
    let query_lower = query.to_lowercase();
    let results: Vec<serde_json::Value> = all
        .iter()
        .filter(|m| m.content.to_lowercase().contains(&query_lower))
        .take(limit)
        .map(memory_to_json)
        .collect();
    Json(serde_json::json!({
        "memories": results,
        "count": results.len(),
        "has_more": false,
    }))
    .into_response()
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
