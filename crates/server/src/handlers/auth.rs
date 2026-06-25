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

/// 严格鉴权：仅接受 `Authorization: Bearer <token>` Header。
///
/// 之前的实现会接受 `?token=...` URL 参数，导致 token 写入浏览器历史、
/// nginx access log、代理日志、上游 CDN 缓存等可观察位置，属于主动泄露。
/// 现彻底移除 query 路径，无论 URL 是否带 token，都必须走 Header。
pub(crate) fn authenticate_admin_token(
    headers: &axum::http::HeaderMap,
    expected_read: &str,
    expected_write: &str,
    require_write: bool,
) -> bool {
    let Some(auth_header) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(auth_str) = auth_header.to_str() else {
        return false;
    };
    let Some(token) = auth_str.strip_prefix("Bearer ") else {
        return false;
    };
    check_token_value(token, expected_read, expected_write, require_write)
}

/// 验证读权限 (R)
///
/// 允许 R 或 RW Token；只接受 Authorization Header。
pub async fn require_read_token(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    info!(
        "require_read_token called: uri={}",
        req.uri()
    );

    if authenticate_admin_token(
        req.headers(),
        &state.admin_read_token,
        &state.admin_write_token,
        false,
    ) {
        Ok(next.run(req).await)
    } else {
        warn!("Read access denied: Invalid or missing Bearer token");
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// 验证读写权限 (RW)
///
/// 仅允许 RW Token；只接受 Authorization Header。
pub async fn require_write_token(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    info!(
        "require_write_token called: uri={}",
        req.uri()
    );

    if authenticate_admin_token(
        req.headers(),
        &state.admin_read_token,
        &state.admin_write_token,
        true,
    ) {
        Ok(next.run(req).await)
    } else {
        warn!("Write access denied: Invalid or missing Bearer token");
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn check_token_value(
    token: &str,
    expected_read: &str,
    expected_write: &str,
    require_write: bool,
) -> bool {
    info!(
        "Checking token: provided={}, require_write={}",
        mask_token(token),
        require_write
    );

    // RW Token 拥有所有权限
    if token == expected_write {
        info!("Token authenticated as WRITE token");
        return true;
    }

    // 如果不需要写权限，R Token 也可以
    if !require_write && token == expected_read {
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

/// 验证设备令牌（Agent 提交 proposal 使用）
///
/// 从 Authorization: Bearer <token> 提取 auth_token，
/// 按 auth_token 查库校验设备身份。
pub async fn require_device_token(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let token = match token {
        Some(t) => t,
        None => {
            warn!("Device auth failed: missing Bearer token");
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    match crate::db::find_device_by_auth_token(&state.db_pool, &token).await {
        Ok(Some(_device_id)) => {
            info!("Device authenticated: token={}", mask_token(&token));
            Ok(next.run(req).await)
        }
        Ok(None) => {
            warn!("Device auth failed: invalid token");
            Err(StatusCode::UNAUTHORIZED)
        }
        Err(e) => {
            warn!("Device auth error: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::authenticate_admin_token;
    use axum::http::{HeaderMap, HeaderValue, header};

    const READ_TOKEN: &str = "r-token-aaaaaaaaaaaaaaaa";
    const WRITE_TOKEN: &str = "w-token-bbbbbbbbbbbbbbbb";

    fn headers_with_bearer(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        h
    }

    /// 验证 P1-20：合法 Bearer Header 必须能过 RW 鉴权。
    #[test]
    fn test_authenticate_admin_token_accepts_bearer_header() {
        let h = headers_with_bearer(WRITE_TOKEN);
        assert!(authenticate_admin_token(&h, READ_TOKEN, WRITE_TOKEN, true));
        assert!(authenticate_admin_token(&h, READ_TOKEN, WRITE_TOKEN, false));
    }

    /// 验证 P1-20：读权限允许 R 或 RW。
    #[test]
    fn test_authenticate_admin_token_read_accepts_read_or_write() {
        let h_read = headers_with_bearer(READ_TOKEN);
        let h_write = headers_with_bearer(WRITE_TOKEN);
        assert!(authenticate_admin_token(&h_read, READ_TOKEN, WRITE_TOKEN, false));
        assert!(authenticate_admin_token(&h_write, READ_TOKEN, WRITE_TOKEN, false));
    }

    /// 验证 P1-20：写权限只允许 RW；R Token 必须被拒。
    #[test]
    fn test_authenticate_admin_token_write_rejects_read_token() {
        let h_read = headers_with_bearer(READ_TOKEN);
        assert!(!authenticate_admin_token(&h_read, READ_TOKEN, WRITE_TOKEN, true));
    }

    /// 验证 P1-20：缺 Header 直接拒绝。
    /// 即便 URL 拼了 `?token=...`，也不会被接受 —— 这是 P1-20 修复的核心契约。
    #[test]
    fn test_authenticate_admin_token_rejects_missing_header() {
        let h = HeaderMap::new();
        assert!(!authenticate_admin_token(&h, READ_TOKEN, WRITE_TOKEN, false));
        assert!(!authenticate_admin_token(&h, READ_TOKEN, WRITE_TOKEN, true));
    }

    /// 验证 P1-20：错值 Header 必须被拒。
    #[test]
    fn test_authenticate_admin_token_rejects_invalid_value() {
        let h = headers_with_bearer("not-a-token");
        assert!(!authenticate_admin_token(&h, READ_TOKEN, WRITE_TOKEN, false));
        assert!(!authenticate_admin_token(&h, READ_TOKEN, WRITE_TOKEN, true));
    }

    /// 验证 P1-20：非 Bearer 前缀必须被拒。
    #[test]
    fn test_authenticate_admin_token_rejects_non_bearer_scheme() {
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic dXNlcjpwYXNz"),
        );
        assert!(!authenticate_admin_token(&h, READ_TOKEN, WRITE_TOKEN, false));
    }
}
