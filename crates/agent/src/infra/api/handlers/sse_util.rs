// ============================================================================
// SSE 工具（agent HTTP API 的 SSE 端点共享）
// ============================================================================
//
// 帧格式与响应头的唯一事实源；`/api/v1/state/stream` 与 `/api/v1/events`
// 两个端点共用，禁止各自手写帧拼接。

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use bytes::Bytes;

/// 空闲心跳间隔（client 契约：30s 一次）
pub(crate) const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// SSE 数据帧（event: <event>\ndata: <json>\n\n）
pub(crate) fn sse_frame(event: &str, json: &str) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {json}\n\n"))
}

/// SSE 响应（text/event-stream + no-cache / keep-alive / 禁代理缓冲）
pub(crate) fn sse_response<B>(body: B) -> Response
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<axum::BoxError>,
{
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .header("X-Accel-Buffering", "no")
        .body(Body::new(body))
        .expect("valid HTTP response")
}
