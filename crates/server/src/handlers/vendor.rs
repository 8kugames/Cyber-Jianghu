// ============================================================================
// Vendor 补货规则 API
// ============================================================================

use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{error, info};
use uuid::Uuid;

use crate::state::AppState;

// ============================================================================
// 请求/响应类型
// ============================================================================

/// 设置补货规则请求
#[derive(Debug, Deserialize)]
pub struct SetRefillRequest {
    pub item_id: String,
    pub threshold: i32,
    pub refill_to: i32,
    pub budget_ratio: i32,
}

/// 切换启用状态请求
#[derive(Debug, Deserialize)]
pub struct ToggleRefillRequest {
    pub enabled: bool,
}

// ============================================================================
// Handler
// ============================================================================

/// GET /api/dashboard/agent/{id}/vendor-refill
pub async fn get_vendor_refill_rules(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
) -> Json<Vec<crate::db::VendorRefillRule>> {
    match crate::db::get_vendor_refills(&state.db_pool, agent_id).await {
        Ok(rules) => Json(rules),
        Err(e) => {
            error!("获取补货规则失败: agent_id={}, error={}", agent_id, e);
            Json(vec![])
        }
    }
}

/// PUT /api/dashboard/agent/{id}/vendor-refill
pub async fn set_vendor_refill_rule(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<SetRefillRequest>,
) -> Result<Json<crate::db::VendorRefillRule>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    // 验证物品存在
    if !crate::game_data::registry::ItemRegistry::exists(&payload.item_id) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("物品 '{}' 不存在", payload.item_id)})),
        ));
    }
    if payload.threshold <= 0
        || payload.refill_to <= payload.threshold
        || payload.budget_ratio <= 0
        || payload.budget_ratio > 100
    {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"error": "参数不合法: threshold>0, refill_to>threshold, budget_ratio 1-100"}),
            ),
        ));
    }

    info!(
        "设置补货规则: agent={}, item={}, threshold={}, refill_to={}, budget={}%",
        agent_id, payload.item_id, payload.threshold, payload.refill_to, payload.budget_ratio
    );

    match crate::db::set_vendor_refill(
        &state.db_pool,
        agent_id,
        &payload.item_id,
        payload.threshold,
        payload.refill_to,
        payload.budget_ratio,
    )
    .await
    {
        Ok(rule) => Ok(Json(rule)),
        Err(e) => {
            error!("设置补货规则失败: {}", e);
            Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "设置失败"})),
            ))
        }
    }
}

/// DELETE /api/dashboard/agent/{id}/vendor-refill/{item_id}
pub async fn delete_vendor_refill_rule(
    State(state): State<Arc<AppState>>,
    Path((agent_id, item_id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    info!("删除补货规则: agent={}, item={}", agent_id, item_id);

    match crate::db::remove_vendor_refill(&state.db_pool, agent_id, &item_id).await {
        Ok(true) => Ok(Json(serde_json::json!({"success": true}))),
        Ok(false) => Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "规则不存在"})),
        )),
        Err(e) => {
            error!("删除补货规则失败: {}", e);
            Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "删除失败"})),
            ))
        }
    }
}
