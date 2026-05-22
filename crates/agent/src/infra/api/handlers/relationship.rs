// 关系 API Handlers
// ============================================================================

use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tracing::error;
use uuid::Uuid;

use super::HttpApiState;
use super::dto::RelationshipUpdateRequest;
use super::service::RelationshipService;

/// 获取所有关系
pub(crate) async fn get_relationships_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let store_arc = state.relationship_store.read().expect("rwlock poisoned").clone();
    let store = match store_arc.as_ref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Relationship store not initialized",
            )
                .into_response();
        }
    };

    let service = RelationshipService::new(store);
    match service.get_all() {
        Ok(relationships) => {
            Json(serde_json::json!({ "relationships": relationships })).into_response()
        }
        Err(e) => {
            error!("[http] Failed to get relationships: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get relationships: {}", e),
            )
                .into_response()
        }
    }
}

/// 获取特定关系
pub(crate) async fn get_relationship_handler(
    State(state): State<HttpApiState>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let store_arc = state.relationship_store.read().expect("rwlock poisoned").clone();
    let store = match store_arc.as_ref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Relationship store not initialized",
            )
                .into_response();
        }
    };

    let target_id = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid UUID format").into_response(),
    };

    let service = RelationshipService::new(store);
    match service.get(target_id) {
        Ok(Some(relationship)) => Json(relationship).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Relationship not found").into_response(),
        Err(e) => {
            error!("[http] Failed to get relationship: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get relationship: {}", e),
            )
                .into_response()
        }
    }
}

/// 更新关系
pub(crate) async fn update_relationship_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<RelationshipUpdateRequest>,
) -> impl IntoResponse {
    let store_arc = state.relationship_store.read().expect("rwlock poisoned").clone();
    let store = match store_arc.as_ref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Relationship store not initialized",
            )
                .into_response();
        }
    };

    let target_id = match Uuid::parse_str(&req.target_agent_id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Invalid target_agent_id format").into_response();
        }
    };

    let tick_id = state
        .current_state
        .read()
        .await
        .as_ref()
        .map(|s| s.tick_id)
        .unwrap_or(0);

    let event = match (&req.event_type, &req.event_description) {
        (Some(event_type), Some(description)) => Some((
            event_type.clone(),
            description.clone(),
            req.event_favorability_delta.unwrap_or(0),
            tick_id,
        )),
        _ => None,
    };

    let service = RelationshipService::new(store);
    match service.update(target_id, &req.target_name, req.favorability_delta, event) {
        Ok(_) => (StatusCode::OK, "Relationship updated").into_response(),
        Err(e) => {
            error!("[http] Failed to update relationship: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to update relationship: {}", e),
            )
                .into_response()
        }
    }
}

// ============================================================================
