use axum::{
    Json,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use std::net::SocketAddr;
use std::sync::Arc;

use crate::state::AppState;

#[derive(Debug, serde::Deserialize)]
pub struct LoginRequest {
    pub token: String,
}

#[derive(Debug, serde::Serialize)]
pub struct LoginResponse {
    pub authenticated: bool,
    pub token_type: String,
}

pub async fn check_session(State(state): State<Arc<AppState>>, req: Request) -> impl IntoResponse {
    // P1-20 修复：仅接受 Authorization Header；不再接受 URL query token。
    // 之前 query fallback 会把 token 写入浏览器历史、access log、CDN 缓存。
    let token = extract_bearer_token(&req);

    if let Some(token) = token {
        // 常量时间比对，避免计时侧信道（P1-20: admin token 不再用裸 ==）
        if crate::handlers::auth::constant_time_eq(token.as_bytes(), state.admin_write_token.as_bytes()) {
            return Json(serde_json::json!({
                "authenticated": true,
                "token_type": "write"
            }))
            .into_response();
        }
        if crate::handlers::auth::constant_time_eq(token.as_bytes(), state.admin_read_token.as_bytes()) {
            return Json(serde_json::json!({
                "authenticated": true,
                "token_type": "read"
            }))
            .into_response();
        }
    }

    Json(serde_json::json!({
        "authenticated": false
    }))
    .into_response()
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
    let token = body.token.trim();

    let token_type = if crate::handlers::auth::constant_time_eq(
        token.as_bytes(),
        state.admin_write_token.as_bytes(),
    ) {
        "write"
    } else if crate::handlers::auth::constant_time_eq(
        token.as_bytes(),
        state.admin_read_token.as_bytes(),
    ) {
        "read"
    } else {
        if let Err(e) = crate::db::insert_audit_log(
            &state.db_pool,
            crate::db::AuditLogEntry {
                event_type: "admin.login",
                actor_type: "admin",
                token_type: None,
                resource_type: "admin_session",
                resource_id: None,
                endpoint: "/api/admin/login",
                method: "POST",
                result: "failure",
                reason: Some("invalid_token".to_string()),
                payload: serde_json::json!({}),
                request_id: Some(audit_ctx.request_id.clone()),
                ip: audit_ctx.ip.clone(),
                user_agent: audit_ctx.user_agent.clone(),
                before_state: None,
                after_state: None,
            },
        )
        .await
        {
            tracing::error!("audit_log 写入失败(admin.login failure): {}", e);
        }
        tracing::warn!("Admin login failed: invalid token provided");
        return Err(StatusCode::UNAUTHORIZED);
    };

    if let Err(e) = crate::db::insert_audit_log(
        &state.db_pool,
        crate::db::AuditLogEntry {
            event_type: "admin.login",
            actor_type: "admin",
            token_type: Some(token_type),
            resource_type: "admin_session",
            resource_id: None,
            endpoint: "/api/admin/login",
            method: "POST",
            result: "success",
            reason: None,
            payload: serde_json::json!({ "token_type": token_type }),
            request_id: Some(audit_ctx.request_id.clone()),
            ip: audit_ctx.ip.clone(),
            user_agent: audit_ctx.user_agent.clone(),
            before_state: None,
            after_state: None,
        },
    )
    .await
    {
        tracing::error!("audit_log 写入失败(admin.login success): {}", e);
    }

    tracing::info!("Admin login successful: token_type={}", token_type);

    Ok(Json(LoginResponse {
        authenticated: true,
        token_type: token_type.to_string(),
    }))
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
    if let Err(e) = crate::db::insert_audit_log(
        &state.db_pool,
        crate::db::AuditLogEntry {
            event_type: "admin.logout",
            actor_type: "admin",
            token_type: None,
            resource_type: "admin_session",
            resource_id: None,
            endpoint: "/api/admin/logout",
            method: "POST",
            result: "success",
            reason: None,
            payload: serde_json::json!({}),
            request_id: Some(audit_ctx.request_id),
            ip: audit_ctx.ip,
            user_agent: audit_ctx.user_agent,
            before_state: None,
            after_state: None,
        },
    )
    .await
    {
        tracing::error!("audit_log 写入失败(admin.logout): {}", e);
    }

    Json(serde_json::json!({
        "message": "Logged out. Token auth does not require server-side session."
    }))
}

fn extract_bearer_token(req: &Request) -> Option<String> {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}
