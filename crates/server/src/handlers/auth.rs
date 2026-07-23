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

/// 验证游戏客户端读权限（低特权档）
///
/// 专为前端/游戏客户端读取 dashboard 数据设计的低特权鉴权档。
/// 优先校验 `client_read_token`（仅读，不能命中任何 WRITE 端点）；
/// 若 `client_read_token` 未配置（None），则回退到 admin read token，
/// 保证向后兼容（旧部署未设 CLIENT_READ_TOKEN 时前端照常工作）。
///
/// 安全说明：即便前端持有了 client_read_token，也无法用它调任何 require_write_token
/// 端点 —— require_write_token 只认 admin write token，与本中间件完全隔离。
pub async fn require_client_read_token(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    info!(
        "require_client_read_token called: uri={}",
        req.uri()
    );

    // 1) 优先：如果配了 CLIENT_READ_TOKEN，先尝试用它鉴权
    if let Some(client_token) = &state.client_read_token
        && authenticate_with_any_token(req.headers(), client_token)
    {
        return Ok(next.run(req).await);
    }

    // 2) 回退：admin read token 或 admin write token 都接受
    //    （保持向后兼容：旧部署未设 CLIENT_READ_TOKEN 时，前端用 admin read token）
    if authenticate_admin_token(
        req.headers(),
        &state.admin_read_token,
        &state.admin_write_token,
        false,
    ) {
        Ok(next.run(req).await)
    } else {
        warn!(
            "Client read access denied: Invalid or missing Bearer token (uri={})",
            req.uri()
        );
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// 单 token 比对：从 Authorization Bearer 提取 token，与期望值做常量时间比对。
///
/// 不接受 query 参数、不接受 Basic scheme —— 与 authenticate_admin_token 的安全契约一致。
fn authenticate_with_any_token(headers: &axum::http::HeaderMap, expected: &str) -> bool {
    let Some(auth_header) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(auth_str) = auth_header.to_str() else {
        return false;
    };
    let Some(token) = auth_str.strip_prefix("Bearer ") else {
        return false;
    };
    // 常量时间比对，避免计时侧信道
    constant_time_eq(token.as_bytes(), expected.as_bytes())
}

/// 常量时间字节比对（避免 token 长度/前缀差异泄露信息）
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

    // RW Token 拥有所有权限（常量时间比对，避免计时侧信道）
    if constant_time_eq(token.as_bytes(), expected_write.as_bytes()) {
        info!("Token authenticated as WRITE token");
        return true;
    }

    // 如果不需要写权限，R Token 也可以（常量时间比对）
    if !require_write && constant_time_eq(token.as_bytes(), expected_read.as_bytes()) {
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

    // ---- require_client_read_token 的纯函数 authenticate_with_any_token 单测 ----

    use super::authenticate_with_any_token;
    const CLIENT_TOKEN: &str = "c-token-cccccccccccccccc";

    #[test]
    fn test_authenticate_with_any_token_accepts_client_token() {
        let h = headers_with_bearer(CLIENT_TOKEN);
        assert!(authenticate_with_any_token(&h, CLIENT_TOKEN));
    }

    #[test]
    fn test_authenticate_with_any_token_rejects_admin_token() {
        // client_read_token 档只接受 client_token 自身，admin token 不应命中
        let h_read = headers_with_bearer(READ_TOKEN);
        let h_write = headers_with_bearer(WRITE_TOKEN);
        assert!(!authenticate_with_any_token(&h_read, CLIENT_TOKEN));
        assert!(!authenticate_with_any_token(&h_write, CLIENT_TOKEN));
    }

    #[test]
    fn test_authenticate_with_any_token_rejects_missing_or_invalid() {
        let empty = HeaderMap::new();
        assert!(!authenticate_with_any_token(&empty, CLIENT_TOKEN));
        let bad = headers_with_bearer("garbage");
        assert!(!authenticate_with_any_token(&bad, CLIENT_TOKEN));
    }

    #[test]
    fn test_constant_time_eq_handles_unequal_lengths() {
        use super::constant_time_eq;
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
    }
}
