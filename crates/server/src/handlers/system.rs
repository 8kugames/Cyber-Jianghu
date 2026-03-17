use axum::{extract::State, Json};
use std::sync::Arc;

use crate::models::HealthResponse;
use crate::state::AppState;

/// 健康检查接口
///
/// GET /health
///
/// 返回服务端的基本信息，包括版本号和Tick周期
pub async fn health_check(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        tick_duration_secs: state.config.tick_engine.tick_duration_secs,
    })
}

/// 根路径
///
/// GET /
///
/// 返回欢迎信息
pub async fn root() -> &'static str {
    "Cyber-Jianghu Server v0.1.0\n\n天道无为，万物自化。"
}
