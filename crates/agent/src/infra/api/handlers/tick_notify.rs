// Tick 通知 API Handlers
// ============================================================================

use axum::{
    extract::State,
    response::{IntoResponse, Json},
};

use super::HttpApiState;
use super::dto;

/// 获取当前 Tick 状态
///
/// GET /api/v1/tick - 返回当前 tick 状态，用于轮询检测新 tick
pub(crate) async fn get_tick_status_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let current = state.current_state.read().await;
    let agent_id = *state.agent_id.read().await;
    let last_update = state.last_state_update.read().await;

    let (tick_id, has_state, state_tick_id) = match current.as_ref() {
        Some(ws) => (ws.tick_id, true, Some(ws.tick_id)),
        None => (0, false, None),
    };

    let (state_updated_at, state_age_ms) = match *last_update {
        Some(instant) => {
            let age_ms = instant.elapsed().as_millis() as u64;
            // 假设我们不能精确地将 Instant 转为 UTC（因为是单调时钟），
            // 但我们可以用当前 UTC 减去 age_ms 估算。
            let utc_time = chrono::Utc::now() - std::time::Duration::from_millis(age_ms);
            (Some(utc_time.to_rfc3339()), Some(age_ms))
        }
        None => (None, None),
    };

    Json(dto::TickStatusResponse {
        tick_id,
        agent_id: if agent_id.is_nil() {
            None
        } else {
            Some(agent_id.to_string())
        },
        has_new_state: has_state,
        seconds_until_next_tick: None, // 服务端未提供此信息
        last_updated_at: chrono::Utc::now().to_rfc3339(),
        state_tick_id,
        state_updated_at,
        state_age_ms,
    })
}

// ============================================================================
