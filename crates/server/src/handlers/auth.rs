use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;
use tracing::{info, warn};

use crate::state::AppState;

/// 脱敏 token：仅显示前3位和后3位，中间用 *** 替代
/// 短于8位的 token 完全隐藏
fn mask_token(token: &str) -> String {
    if token.len() < 8 {
        "***".to_string()
    } else {
        format!("{}***{}", &token[..3], &token[token.len() - 3..])
    }
}

/// 从 URI 查询参数中提取 token
fn extract_token_from_uri(uri: &axum::http::Uri) -> Option<String> {
    uri.query()
        .and_then(|query| {
            query
                .split('&')
                .find(|pair| pair.starts_with("token="))
                .and_then(|pair| pair.strip_prefix("token="))
        })
        .map(|s| s.to_string())
}

/// 验证读权限 (R)
///
/// 允许 R 或 RW Token
pub async fn require_read_token(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let query_token = extract_token_from_uri(req.uri());

    info!(
        "require_read_token called: token={}, uri={}",
        query_token
            .as_ref()
            .map(|t| mask_token(t))
            .unwrap_or_default(),
        req.uri()
    );

    if check_token(&state, &req, &query_token, false) {
        Ok(next.run(req).await)
    } else {
        warn!("Read access denied: Invalid token");
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// 验证读写权限 (RW)
///
/// 仅允许 RW Token
pub async fn require_write_token(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let query_token = extract_token_from_uri(req.uri());

    info!(
        "require_write_token called: token={}, uri={}",
        query_token
            .as_ref()
            .map(|t| mask_token(t))
            .unwrap_or_default(),
        req.uri()
    );

    if check_token(&state, &req, &query_token, true) {
        Ok(next.run(req).await)
    } else {
        warn!("Write access denied: Invalid token");
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn check_token(
    state: &AppState,
    req: &Request,
    query_token: &Option<String>,
    require_write: bool,
) -> bool {
    // 1. 检查 Query Param
    if let Some(token) = query_token
        && check_token_value(state, token, require_write)
    {
        return true;
    }

    // 2. 检查 Header (Authorization: Bearer <token>)
    if let Some(auth_header) = req.headers().get(header::AUTHORIZATION)
        && let Ok(auth_str) = auth_header.to_str()
        && let Some(token) = auth_str.strip_prefix("Bearer ")
        && check_token_value(state, token, require_write)
    {
        return true;
    }

    false
}

fn check_token_value(state: &AppState, token: &str, require_write: bool) -> bool {
    info!(
        "Checking token: provided={}, require_write={}",
        mask_token(token),
        require_write
    );

    // RW Token 拥有所有权限
    if token == state.admin_write_token {
        info!("Token authenticated as WRITE token");
        return true;
    }

    // 如果不需要写权限，R Token 也可以
    if !require_write && token == state.admin_read_token {
        info!("Token authenticated as READ token");
        return true;
    }

    warn!(
        "Token authentication failed: provided={}, require_write={}",
        mask_token(token),
        require_write
    );
    false
}
