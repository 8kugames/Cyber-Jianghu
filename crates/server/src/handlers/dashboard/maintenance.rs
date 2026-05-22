use axum::{
    Json,
    extract::State,
};
use serde::Serialize;
use std::sync::Arc;

use crate::state::AppState;


#[derive(Serialize)]
pub struct CleanupResult {
    pub deleted_count: u64,
}

/// 清理长期离线的 Agent
pub async fn cleanup_offline_agents(
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    let cleanup_days = {
        let gd_guard = state.game_data.get();
        gd_guard.game_rules.data.ops.offline_cleanup_days
    };

    let mut tx = match state.db_pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("Failed to begin transaction for cleanup: {}", e);
            return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let query_str = format!(
        "DELETE FROM agents WHERE last_tick_online < NOW() - INTERVAL '{} days'",
        cleanup_days
    );

    let result = match sqlx::query(&query_str).execute(&mut *tx).await {
        Ok(res) => res,
        Err(e) => {
            tracing::error!("Failed to execute cleanup query: {}", e);
            return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if let Err(e) = tx.commit().await {
        tracing::error!("Failed to commit transaction for cleanup: {}", e);
        return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    tracing::info!(
        "Dashboard triggered cleanup: deleted {} agents.",
        result.rows_affected()
    );

    Ok(Json(CleanupResult {
        deleted_count: result.rows_affected(),
    }))
}

// ============================================================================
// Agent Experiences API
// ============================================================================

