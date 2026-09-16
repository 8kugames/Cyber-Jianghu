//! 静态文件服务与 HTTP 服务器启动（run_http_server）

use super::*;
use axum::response::IntoResponse;

/// 获取静态文件服务目录
pub fn get_static_serve_dir() -> PathBuf {
    let panel_path = PathBuf::from("crates/agent/static/panel");
    let panel_path_alt = PathBuf::from("static/panel");
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_default();
    let exe_panel_path = exe_dir.join("static/panel");

    if panel_path.exists() {
        panel_path
    } else if panel_path_alt.exists() {
        panel_path_alt
    } else {
        exe_panel_path
    }
}

/// 静态文件 handler：读取磁盘文件并添加 Cache-Control: no-cache 头
async fn static_file_handler(
    req: axum::extract::Request,
    serve_dir: std::path::PathBuf,
) -> axum::response::Response {
    let path = req.uri().path().trim_start_matches('/');
    let file_path = if path.is_empty() || path == "index.html" {
        serve_dir.join("index.html")
    } else {
        serve_dir.join(path)
    };

    // Security: prevent path traversal
    if !file_path.starts_with(&serve_dir) {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }

    match tokio::fs::read(&file_path).await {
        Ok(bytes) => {
            let content_type = match file_path.extension().and_then(|e| e.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("json") => "application/json",
                Some("png") => "image/png",
                Some("jpg") | Some("jpeg") => "image/jpeg",
                Some("svg") => "image/svg+xml",
                Some("ico") => "image/x-icon",
                _ => "application/octet-stream",
            };
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                "cache-control",
                "no-cache, no-store, must-revalidate".parse().unwrap(),
            );
            headers.insert("content-type", content_type.parse().unwrap());
            (axum::http::StatusCode::OK, headers, bytes).into_response()
        }
        Err(_) => {
            // SPA fallback: serve index.html for unknown routes
            match tokio::fs::read(serve_dir.join("index.html")).await {
                Ok(html) => {
                    let mut headers = axum::http::HeaderMap::new();
                    headers.insert(
                        "cache-control",
                        "no-cache, no-store, must-revalidate".parse().unwrap(),
                    );
                    headers.insert("content-type", "text/html; charset=utf-8".parse().unwrap());
                    (axum::http::StatusCode::OK, headers, html).into_response()
                }
                Err(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
            }
        }
    }
}

/// 启动 HTTP API 服务器
///
/// 启动后监听指定端口，提供 RESTful API 供外部系统调用
/// 所有端点都需要从共享状态中获取对应的 AI 组件，如果组件未初始化
/// 则返回 503 SERVICE_UNAVAILABLE 错误
pub async fn run_http_server(port: u16, api_state: HttpApiState) -> anyhow::Result<()> {
    let static_dir = get_static_serve_dir();
    // 所有 API 端点必须携带有效 device auth_token。
    // 镜像 server 端 `require_*_token` 模式。白名单（health/静态资源）由中间件内部判定。
    let app = create_api_router()
        .layer(axum::middleware::from_fn_with_state(
            api_state.clone(),
            auth::require_device_token,
        ))
        .with_state(api_state.clone())
        .fallback(move |req: axum::extract::Request| static_file_handler(req, static_dir.clone()));

    let addr = format!("0.0.0.0:{}", port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            return Err(anyhow::anyhow!(
                "无法绑定端口 {} ({}): 请检查是否有旧进程仍在运行，或使用其他端口",
                port,
                e
            ));
        }
    };
    let local_addr = listener.local_addr()?;
    info!("[http] API Server listening on {}", local_addr);
    info!("[http] HTTP_PORT={}", local_addr.port());
    info!("[http] Web Panel: http://0.0.0.0:{}/", local_addr.port());
    info!(
        "[http] - Dashboard:       http://0.0.0.0:{}/#/dashboard",
        local_addr.port()
    );
    info!(
        "[http] - Character info:  http://0.0.0.0:{}/#/characters",
        local_addr.port()
    );
    info!(
        "[http] - Settings:        http://0.0.0.0:{}/#/settings",
        local_addr.port()
    );

    // connect_info：setup/status 据此做 loopback 信任判定（auth_token 暴露门控）
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

// ============================================================================
// 默认对话处理器
// ============================================================================
