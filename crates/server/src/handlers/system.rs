use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use std::sync::Arc;

use crate::models::{DbHealthStatus, HealthResponse};
use crate::state::AppState;

/// 健康检查接口
///
/// GET /health
///
/// 返回服务端的基本信息，包括版本号和Tick周期
pub async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Get tick duration from game_data
    let tick_duration_secs = {
        let gd = state.game_data.get();
        gd.game_rules.data.agent_state.tick.real_seconds_per_tick as u64
    };

    let db_result = sqlx::query("SELECT 1").execute(&state.db_pool).await;
    let db_ok = db_result.is_ok();
    let now = chrono::Utc::now();
    {
        let mut health = state
            .db_runtime_health
            .write()
            .expect("db runtime health lock poisoned");
        crate::db::record_db_probe_result(
            &mut health,
            db_ok,
            now,
            db_result.err().map(|e| e.to_string()),
        );
    }
    let db_health = state
        .db_runtime_health
        .read()
        .expect("db runtime health lock poisoned")
        .clone();
    let status = if db_ok && db_health.is_available {
        "ok"
    } else {
        "db_unavailable"
    };
    let response = Json(HealthResponse {
        status: status.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        tick_duration_secs,
        db: DbHealthStatus {
            current_query_ok: db_ok,
            probe_available: db_health.is_available,
            last_probe_at: db_health.last_probe_at,
            last_failure_at: db_health.last_failure_at,
            last_recovery_at: db_health.last_recovery_at,
            last_error: db_health.last_error,
        },
    });

    if db_ok {
        (StatusCode::OK, response).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, response).into_response()
    }
}

/// 根路径
///
/// GET /
///
/// 返回欢迎信息
pub async fn root() -> &'static str {
    "Cyber-Jianghu Server v0.1.0\n\n天道无为，万物自化。"
}
