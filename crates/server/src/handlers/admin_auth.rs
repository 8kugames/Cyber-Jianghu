use axum::{
    Json,
    extract::{Request, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
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
    let token = extract_bearer_token(&req).or_else(|| extract_query_token(&req));

    if let Some(token) = token {
        if token == state.admin_write_token {
            return Json(serde_json::json!({
                "authenticated": true,
                "token_type": "write"
            }))
            .into_response();
        }
        if token == state.admin_read_token {
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
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    let token = body.token.trim();

    let token_type = if token == state.admin_write_token {
        "write"
    } else if token == state.admin_read_token {
        "read"
    } else {
        tracing::warn!("Admin login failed: invalid token provided");
        return Err(StatusCode::UNAUTHORIZED);
    };

    tracing::info!("Admin login successful: token_type={}", token_type);

    Ok(Json(LoginResponse {
        authenticated: true,
        token_type: token_type.to_string(),
    }))
}

pub async fn logout() -> impl IntoResponse {
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

fn extract_query_token(req: &Request) -> Option<String> {
    req.uri()
        .query()
        .and_then(|query| {
            query
                .split('&')
                .find(|pair| pair.starts_with("token="))
                .and_then(|pair| pair.strip_prefix("token="))
        })
        .map(|s| s.to_string())
}
