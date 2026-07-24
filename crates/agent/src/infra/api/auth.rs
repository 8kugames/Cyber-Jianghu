//! HTTP API 认证中间件
//!
//! 背景（P0-11(b)）：Agent HTTP API 之前完全无认证。bind 已改为 127.0.0.1（P0-11(a)），
//! 但本机任何进程仍可调用 `POST /api/v1/config/llm` 改 LLM endpoint（玩家大脑劫持）。
//!
//! 修复：镜像 server 端 `axum::middleware::from_fn_with_state` + `Authorization: Bearer <token>`
//! 模式。token 复用 `HttpApiState.device_config.auth_token`（设备向 server 注册时获得）。
//!
//! 白名单：`/api/v1/health`、`/api/v1`（API 列表）、`/`（静态面板首页）、静态资源。
//! 这些是浏览器首次加载 + 健康检查所需的公开端点。
//!
//! fail-closed 策略：`device_config` 未初始化（启动早期）时返 503，强制先完成设备注册。

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use tracing::{debug, warn};

use super::HttpApiState;

/// 不需要认证的公开路径前缀/精确匹配。
///
/// - `/`：静态面板首页（浏览器加载 SPA）
/// - `/api/v1`、`/api/v1/`：API 根（端点列表），不含敏感数据
/// - `/api/v1/health`：健康检查（容器编排探针用）
/// - `/api/v1/setup`：设备注册引导（未认证前的引导端点，若存在）
/// - 静态资源：CSS/JS/图标（`/static/`、`/assets/`、`.js`、`.css`、`.ico` 等）
pub fn is_public_path(path: &str) -> bool {
    // 精确匹配
    if path == "/" || path == "/api/v1" || path == "/api/v1/" || path == "/api/v1/health" {
        return true;
    }
    // setup 引导路径前缀
    if path.starts_with("/api/v1/setup") {
        return true;
    }
    // 静态资源：非 /api/v1/ 开头的路径都是静态文件（SPA 路由 + 资源）
    // 静态面板的路由是 hash-based（/#/dashboard），故服务端只见 /、/static/、文件名
    if !path.starts_with("/api/") {
        return true;
    }
    false
}

/// 从 `Authorization: Bearer <token>` header 提取 token。
///
/// 返回 `None` 表示 header 缺失或格式错误（非 Bearer scheme）。
pub fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
    let header_value = headers.get(axum::http::header::AUTHORIZATION)?;
    let value = header_value.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    let trimmed = token.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// 检查请求是否通过认证。
///
/// 返回 `Ok(())` 表示通过（或为公开路径），返回 `Err(StatusCode)` 表示拒绝。
///
/// - 公开路径 → `Ok(())`
/// - `device_config` 未初始化 → `Err(503)`（fail-closed：启动早期不该有业务流量）
/// - 无 Authorization header 且无合法 query token → `Err(401)`
/// - token 不匹配 → `Err(401)`
/// - token 匹配 → `Ok(())`
///
/// `uri` 为完整 URI（含 query），用于支持 SSE 端点的 query token。
/// 浏览器 `EventSource` 不支持自定义 header，故对 `/api/v1/events` 额外接受
/// `?token=<token>` 作为 Bearer 的等价物。query token 仅对 SSE 端点生效。
pub fn check_auth(
    expected_token: Option<&str>,
    headers: &HeaderMap,
    path: &str,
    uri: &axum::http::Uri,
) -> Result<(), StatusCode> {
    if is_public_path(path) {
        return Ok(());
    }

    let expected = expected_token.ok_or_else(|| {
        warn!(
            "P0-11(b) 认证拒绝：device_config 未初始化，path={}（fail-closed: 503）",
            path
        );
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    // 优先 Bearer header（普通 fetch 路径）
    if let Some(provided) = extract_bearer_token(headers) {
        if provided == expected {
            return Ok(());
        }
        warn!(
            "P0-11(b) 认证拒绝：token 不匹配，path={}（提供的前 4 字符：{:?}）",
            path,
            provided.chars().take(4).collect::<String>()
        );
        return Err(StatusCode::UNAUTHORIZED);
    }

    // SSE 端点退化接受 query token（EventSource 无法带 header）
    // 注意：仅 /api/v1/events 开放此通道，且仅作 Bearer 的等价物。
    // 前端用 encodeURIComponent(token) 编码（app.js），后端须 percent-decode 后比较，
    // 否则含 + / / = 等保留字符的 token 格式会静默失配（SSE 永久 401）。
    if path == "/api/v1/events"
        && let Some(q) = uri.query()
    {
        for pair in q.split('&') {
            if let Some(raw) = pair.strip_prefix("token=") {
                // query token 经 encodeURIComponent 编码，这里解码后与明文 expected 比较
                let provided = percent_encoding::percent_decode_str(raw)
                    .decode_utf8()
                    .map(|cow| cow.into_owned())
                    .unwrap_or_default();
                if provided == expected {
                    return Ok(());
                }
                warn!(
                    "P0-11(b) 认证拒绝（SSE query token）：token 不匹配，path={}",
                    path
                );
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    }

    debug!(
        "P0-11(b) 认证拒绝：缺少或格式错误的 Authorization header，path={}",
        path
    );
    Err(StatusCode::UNAUTHORIZED)
}

/// axum 中间件：要求请求携带有效的 device auth_token。
///
/// 镜像 server 端 `handlers/auth.rs::require_*_token` 模式：
/// `async fn(State, req: Request, next: Next) -> Result<Response, StatusCode>`。
/// 通过 `axum::middleware::from_fn_with_state(api_state, require_device_token)` 应用。
pub async fn require_device_token(
    State(state): State<HttpApiState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let headers = req.headers().clone();
    // 在 device_config 读锁存活期间完成 check_auth（token 引用从 guard 借出）
    let auth_result = {
        let guard = state.device_config.read().await;
        let expected_token = guard.as_ref().map(|c| c.auth_token.as_str());
        check_auth(expected_token, &headers, &path, &uri)
    };
    match auth_result {
        Ok(()) => Ok(next.run(req).await),
        Err(status) => {
            warn!(
                "P0-11(b) API 认证拒绝: path={}, status={}",
                path, status
            );
            Err(status)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    // =========================================================================
    // is_public_path
    // =========================================================================

    #[test]
    fn test_is_public_path_whitelists_root_and_health() {
        assert!(is_public_path("/"));
        assert!(is_public_path("/api/v1"));
        assert!(is_public_path("/api/v1/"));
        assert!(is_public_path("/api/v1/health"));
    }

    #[test]
    fn test_is_public_path_whitelists_setup_prefix() {
        assert!(is_public_path("/api/v1/setup"));
        assert!(is_public_path("/api/v1/setup/status"));
    }

    #[test]
    fn test_is_public_path_whitelists_static_resources() {
        // 非 /api/ 开头都是静态资源（SPA + 文件）
        assert!(is_public_path("/static/js/main.js"));
        assert!(is_public_path("/assets/style.css"));
        assert!(is_public_path("/favicon.ico"));
        assert!(is_public_path("/index.html"));
    }

    #[test]
    fn test_is_public_path_rejects_protected_endpoints() {
        assert!(!is_public_path("/api/v1/config/llm"));
        assert!(!is_public_path("/api/v1/character/register"));
        assert!(!is_public_path("/api/v1/state"));
        assert!(!is_public_path("/api/v1/intent"));
    }

    // =========================================================================
    // extract_bearer_token
    // =========================================================================

    #[test]
    fn test_extract_bearer_token_valid() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer abc123token"),
        );
        assert_eq!(extract_bearer_token(&headers), Some("abc123token"));
    }

    #[test]
    fn test_extract_bearer_token_missing_header() {
        let headers = HeaderMap::new();
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn test_extract_bearer_token_wrong_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Basic abc123"));
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn test_extract_bearer_token_empty_token() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer "));
        assert_eq!(extract_bearer_token(&headers), None);
    }

    // =========================================================================
    // check_auth
    // =========================================================================

    #[test]
    fn test_check_auth_public_path_always_passes() {
        let headers = HeaderMap::new();
        let uri = "/api/v1/health".parse::<axum::http::Uri>().unwrap();
        // 无 token，但公开路径 → 通过
        assert_eq!(check_auth(None, &headers, "/api/v1/health", &uri), Ok(()));
        let uri = "/".parse::<axum::http::Uri>().unwrap();
        assert_eq!(check_auth(None, &headers, "/", &uri), Ok(()));
    }

    #[test]
    fn test_check_auth_rejects_when_device_not_configured() {
        let headers = HeaderMap::new();
        let uri = "/api/v1/config/llm"
            .parse::<axum::http::Uri>()
            .unwrap();
        // 受保护路径 + device_config=None → 503 fail-closed
        assert_eq!(
            check_auth(None, &headers, "/api/v1/config/llm", &uri),
            Err(StatusCode::SERVICE_UNAVAILABLE)
        );
    }

    #[test]
    fn test_check_auth_rejects_missing_header() {
        let headers = HeaderMap::new();
        let uri = "/api/v1/config/llm"
            .parse::<axum::http::Uri>()
            .unwrap();
        assert_eq!(
            check_auth(Some("secret"), &headers, "/api/v1/config/llm", &uri),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn test_check_auth_rejects_wrong_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer wrong-token"),
        );
        let uri = "/api/v1/config/llm"
            .parse::<axum::http::Uri>()
            .unwrap();
        assert_eq!(
            check_auth(Some("correct-token"), &headers, "/api/v1/config/llm", &uri),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn test_check_auth_accepts_correct_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer correct-token"),
        );
        let uri = "/api/v1/config/llm"
            .parse::<axum::http::Uri>()
            .unwrap();
        assert_eq!(
            check_auth(Some("correct-token"), &headers, "/api/v1/config/llm", &uri),
            Ok(())
        );
    }

    #[test]
    fn test_check_auth_rejects_basic_scheme_on_protected_path() {
        let mut headers = HeaderMap::new();
        // 用 Basic scheme 传 token → 应拒绝（必须是 Bearer）
        headers.insert(
            "authorization",
            HeaderValue::from_static("Basic correct-token"),
        );
        let uri = "/api/v1/config/llm"
            .parse::<axum::http::Uri>()
            .unwrap();
        assert_eq!(
            check_auth(Some("correct-token"), &headers, "/api/v1/config/llm", &uri),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    // =========================================================================
    // SSE query token（/api/v1/events 专用）
    // =========================================================================

    #[test]
    fn test_check_auth_sse_accepts_correct_query_token() {
        // 无 header，但 SSE 端点通过 query token 通过
        let headers = HeaderMap::new();
        let uri = "/api/v1/events?token=my-secret"
            .parse::<axum::http::Uri>()
            .unwrap();
        assert_eq!(
            check_auth(Some("my-secret"), &headers, "/api/v1/events", &uri),
            Ok(())
        );
    }

    #[test]
    fn test_check_auth_sse_rejects_wrong_query_token() {
        let headers = HeaderMap::new();
        let uri = "/api/v1/events?token=wrong"
            .parse::<axum::http::Uri>()
            .unwrap();
        assert_eq!(
            check_auth(Some("correct"), &headers, "/api/v1/events", &uri),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn test_check_auth_sse_rejects_missing_query_token_when_no_header() {
        // SSE 端点既无 header 也无 query token → 401
        let headers = HeaderMap::new();
        let uri = "/api/v1/events".parse::<axum::http::Uri>().unwrap();
        assert_eq!(
            check_auth(Some("correct"), &headers, "/api/v1/events", &uri),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn test_check_auth_query_token_only_accepted_on_sse_endpoint() {
        // 非 SSE 端点带 query token（无 header）→ 仍应 401（query 通道仅对 SSE 开放）
        let headers = HeaderMap::new();
        let uri = "/api/v1/config/llm?token=correct"
            .parse::<axum::http::Uri>()
            .unwrap();
        assert_eq!(
            check_auth(Some("correct"), &headers, "/api/v1/config/llm", &uri),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn test_check_auth_sse_decodes_percent_encoded_query_token() {
        // 前端用 encodeURIComponent(token) 编码；含保留字符（+ / = 等，如 base64 token）的
        // token 经 percent-encoding 后，后端必须解码才能匹配，否则 SSE 永久 401。
        // 这里模拟一个含 + / = 的 base64 风格 token：
        //   明文 "ab+/cd==" → encodeURIComponent → "ab%2B%2Fcd%3D%3D"
        let headers = HeaderMap::new();
        let uri = "/api/v1/events?token=ab%2B%2Fcd%3D%3D"
            .parse::<axum::http::Uri>()
            .unwrap();
        assert_eq!(
            check_auth(Some("ab+/cd=="), &headers, "/api/v1/events", &uri),
            Ok(())
        );
    }
}
