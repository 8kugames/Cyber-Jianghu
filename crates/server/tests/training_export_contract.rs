//! 训练导出跨模块契约测试（无 DB / 无 server 进程）。

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, Request as HttpRequest, StatusCode};
use axum::middleware::{Next, from_fn};
use axum::response::Response;
use axum::routing::{delete, get};
use cyber_jianghu_server::training_export::config::TrainingExportConfig;
use cyber_jianghu_server::training_export::{
    ExportRunRequest, RunMetadata, TriggerSource, validate_run_id,
};
use tower::ServiceExt;

#[test]
fn manual_request_fields_flow_into_metadata() {
    let agent_id = uuid::Uuid::new_v4();
    let request = ExportRunRequest {
        run_id: ulid::Ulid::new().to_string(),
        triggered_by: TriggerSource::Manual,
        agent_id_filter: Some(agent_id),
        force_full: true,
    };

    let metadata = RunMetadata::for_request(&request);
    assert_eq!(metadata.run_id, request.run_id);
    assert_eq!(metadata.triggered_by, TriggerSource::Manual);
    assert_eq!(metadata.agent_id_filter, Some(agent_id));
    assert!(metadata.force_full);
}

#[test]
fn generated_run_id_round_trips_through_ulid_parser() {
    let run_id = ulid::Ulid::new().to_string();
    assert_eq!(validate_run_id(&run_id).unwrap().to_string(), run_id);
}

#[test]
fn path_like_run_ids_are_rejected() {
    for run_id in ["../secret", "foo/bar", "foo\\bar", "", "."] {
        assert!(validate_run_id(run_id).is_err(), "accepted {run_id:?}");
    }
}

#[test]
fn queue_capacity_and_execution_concurrency_are_independent() {
    let config = TrainingExportConfig::default();
    assert_eq!(config.scheduler.max_concurrent_runs, 1);
    assert!(config.scheduler.manual_queue_capacity > 1);
    assert!(config.validate().is_ok());
}

async fn ok_handler() -> StatusCode {
    StatusCode::OK
}

async fn require_read(request: Request, next: Next) -> Result<Response, StatusCode> {
    match request
        .headers()
        .get("x-role")
        .and_then(|value| value.to_str().ok())
    {
        Some("read" | "write") => Ok(next.run(request).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn require_write(request: Request, next: Next) -> Result<Response, StatusCode> {
    match request
        .headers()
        .get("x-role")
        .and_then(|value| value.to_str().ok())
    {
        Some("write") => Ok(next.run(request).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn call_with_role(app: &Router, method: Method, role: Option<&str>) -> StatusCode {
    let mut builder = HttpRequest::builder().method(method).uri("/exports/run-id");
    if let Some(role) = role {
        builder = builder.header("x-role", role);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn merged_method_routes_keep_read_and_write_layers_isolated() {
    let app = Router::new().route(
        "/exports/{run_id}",
        get(ok_handler)
            .layer(from_fn(require_read))
            .merge(delete(ok_handler).layer(from_fn(require_write))),
    );

    assert_eq!(
        call_with_role(&app, Method::GET, Some("read")).await,
        StatusCode::OK
    );
    assert_eq!(
        call_with_role(&app, Method::DELETE, Some("read")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call_with_role(&app, Method::DELETE, Some("write")).await,
        StatusCode::OK
    );
    assert_eq!(
        call_with_role(&app, Method::GET, None).await,
        StatusCode::UNAUTHORIZED
    );
}

#[path = "common/mod.rs"]
mod common;

// 注：real_auth_layer_rejects_anonymous_and_read_token_for_delete 需要构造一个完整
// AppState（17 个字段），其中 UnifiedConfig<T> 等子结构没有派生 Default。直接构造
// `GameDataCache::new(load_game_data().unwrap_or_default())` 会触发 websocket 启动。
// 当前我们用 `merged_method_routes_keep_read_and_write_layers_isolated` 静态证明 axum
// `.merge` 拆 read/write layer 的正确性，再用 crates/server/src/training_export 模块
// 的公共导出让以后的真实 token 测试接入。运行时 token 验证由项目所有者在 staging 验收。
