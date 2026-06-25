// ============================================================================
// Vendor 补货规则 API
// ============================================================================

use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
};
use serde::Deserialize;
use std::net::SocketAddr;
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<SetRefillRequest>,
) -> Result<Json<crate::db::VendorRefillRule>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
    let before_state = crate::db::get_vendor_refills(&state.db_pool, agent_id)
        .await
        .ok()
        .and_then(|rules| rules.into_iter().find(|rule| rule.item_id == payload.item_id))
        .map(|rule| serde_json::json!({
            "threshold": rule.threshold,
            "refill_to": rule.refill_to,
            "budget_ratio": rule.budget_ratio,
            "enabled": rule.enabled,
        }));
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
        Ok(rule) => {
            if let Err(e) = crate::db::insert_audit_log(
                &state.db_pool,
                crate::db::AuditLogEntry {
                    event_type: "vendor.refill.set",
                    actor_type: "admin",
                    token_type: Some("write"),
                    resource_type: "vendor_refill",
                    resource_id: Some(format!("{}:{}", agent_id, payload.item_id)),
                    endpoint: "/api/dashboard/agent/{id}/vendor-refill",
                    method: "PUT",
                    result: "success",
                    reason: None,
                    payload: serde_json::json!({
                        "agent_id": agent_id,
                        "item_id": payload.item_id,
                        "threshold": payload.threshold,
                        "refill_to": payload.refill_to,
                        "budget_ratio": payload.budget_ratio,
                    }),
                    request_id: Some(audit_ctx.request_id),
                    ip: audit_ctx.ip,
                    user_agent: audit_ctx.user_agent,
                    before_state,
                    after_state: Some(serde_json::json!({
                        "threshold": rule.threshold,
                        "refill_to": rule.refill_to,
                        "budget_ratio": rule.budget_ratio,
                        "enabled": rule.enabled,
                    })),
                },
            )
            .await
            {
                error!("audit_log 写入失败(vendor.refill.set): {}", e);
            }
            Ok(Json(rule))
        }
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Path((agent_id, item_id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
    let before_state = crate::db::get_vendor_refills(&state.db_pool, agent_id)
        .await
        .ok()
        .and_then(|rules| rules.into_iter().find(|rule| rule.item_id == item_id))
        .map(|rule| serde_json::json!({
            "threshold": rule.threshold,
            "refill_to": rule.refill_to,
            "budget_ratio": rule.budget_ratio,
            "enabled": rule.enabled,
        }));
    info!("删除补货规则: agent={}, item={}", agent_id, item_id);

    match crate::db::remove_vendor_refill(&state.db_pool, agent_id, &item_id).await {
        Ok(true) => {
            if let Err(e) = crate::db::insert_audit_log(
                &state.db_pool,
                crate::db::AuditLogEntry {
                    event_type: "vendor.refill.delete",
                    actor_type: "admin",
                    token_type: Some("write"),
                    resource_type: "vendor_refill",
                    resource_id: Some(format!("{}:{}", agent_id, item_id)),
                    endpoint: "/api/dashboard/agent/{id}/vendor-refill/{item_id}",
                    method: "DELETE",
                    result: "success",
                    reason: None,
                    payload: serde_json::json!({
                        "agent_id": agent_id,
                        "item_id": item_id,
                    }),
                    request_id: Some(audit_ctx.request_id),
                    ip: audit_ctx.ip,
                    user_agent: audit_ctx.user_agent,
                    before_state,
                    after_state: Some(serde_json::Value::Null),
                },
            )
            .await
            {
                error!("audit_log 写入失败(vendor.refill.delete): {}", e);
            }
            Ok(Json(serde_json::json!({"success": true})))
        }
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
