// 寿命 API Handlers
// ============================================================================

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};

use super::HttpApiState;
use super::dto::LifespanResponse;

/// 获取寿命状态（从 Server 下发的 WorldState 读取）
pub(crate) async fn get_lifespan_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    match current.as_ref() {
        Some(ws) => {
            let age = ws.self_state.age_years.unwrap_or(0) as u8;
            let max_age = ws.self_state.max_age.unwrap_or(80) as u8;
            let is_dead = state.is_dead.load(std::sync::atomic::Ordering::Relaxed);
            Json(LifespanResponse {
                current_age: age,
                status: if is_dead {
                    "deceased"
                } else if age >= max_age {
                    "aging"
                } else {
                    "alive"
                }
                .to_string(),
                aging_effects: None,
            })
            .into_response()
        }
        None => (StatusCode::SERVICE_UNAVAILABLE, "No world state available").into_response(),
    }
}

// ============================================================================
