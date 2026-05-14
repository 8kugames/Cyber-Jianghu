// ============================================================================
// 角色身份管理 API
// ============================================================================

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{error, info};
use uuid::Uuid;

use crate::state::AppState;

pub async fn list_available_roles() -> Json<Vec<String>> {
    Json(crate::game_data::registry::InitialRecipesRegistry::get_roles())
}

// ============================================================================
// 请求/响应类型
// ============================================================================

/// 分配角色请求
#[derive(Debug, Deserialize)]
pub struct AssignRoleRequest {
    pub role_key: String,
}

// ============================================================================
// Handler
// ============================================================================

/// GET /api/dashboard/agent/{id}/roles
pub async fn get_agent_roles_handler(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
) -> Json<Vec<crate::db::AgentRole>> {
    match crate::db::get_agent_roles(&state.db_pool, agent_id).await {
        Ok(roles) => Json(roles),
        Err(e) => {
            error!("获取角色身份失败: agent_id={}, error={}", agent_id, e);
            Json(vec![])
        }
    }
}

/// POST /api/dashboard/agent/{id}/roles
pub async fn assign_role_handler(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<AssignRoleRequest>,
) -> Result<Json<crate::db::AgentRole>, (StatusCode, Json<serde_json::Value>)> {
    // 验证 role_key 存在于 initial_recipes.yaml
    let available_roles = crate::game_data::registry::InitialRecipesRegistry::get_roles();
    if !available_roles.contains(&payload.role_key) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "角色 '{}' 不存在，可用角色: {:?}",
                    payload.role_key, available_roles
                )
            })),
        ));
    }

    // 在 DB 中分配角色
    let role = crate::db::assign_role(&state.db_pool, agent_id, &payload.role_key)
        .await
        .map_err(|e| {
            error!("分配角色失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "分配角色失败"})),
            )
        })?;

    // 将该角色的配方授予 agent_known_recipes
    let recipe_ids =
        crate::game_data::registry::InitialRecipesRegistry::get_role_recipes(&payload.role_key);
    if !recipe_ids.is_empty() {
        let tick_id = crate::db::get_current_world_tick_id(&state.db_pool)
            .await
            .unwrap_or(0);
        if let Err(e) =
            crate::db::assign_initial_recipes(&state.db_pool, agent_id, &recipe_ids, tick_id).await
        {
            error!("授予角色配方失败: {}", e);
        }
    }

    info!(
        "角色身份分配完成: agent={}, role={}, 授予配方={:?}",
        agent_id, payload.role_key, recipe_ids
    );
    Ok(Json(role))
}

/// DELETE /api/dashboard/agent/{id}/roles/{role_key}
pub async fn remove_role_handler(
    State(state): State<Arc<AppState>>,
    Path((agent_id, role_key)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    info!("移除角色身份: agent={}, role={}", agent_id, role_key);

    match crate::db::remove_role(&state.db_pool, agent_id, &role_key).await {
        Ok(true) => Ok(Json(serde_json::json!({"success": true}))),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "角色身份不存在"})),
        )),
        Err(e) => {
            error!("移除角色身份失败: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "移除失败"})),
            ))
        }
    }
}
