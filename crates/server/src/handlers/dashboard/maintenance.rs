use axum::{
    Json,
    extract::{ConnectInfo, State},
};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::state::AppState;

#[derive(Serialize)]
pub struct CleanupResult {
    pub deleted_count: u64,
}

/// 清理长期离线的 Agent
pub async fn cleanup_offline_agents(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> impl axum::response::IntoResponse {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
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

    if let Err(e) = crate::db::insert_audit_log(
        &state.db_pool,
        crate::db::AuditLogEntry {
            event_type: "agent.cleanup_offline",
            actor_type: "admin",
            token_type: Some("write"),
            resource_type: "agent",
            resource_id: None,
            endpoint: "/api/dashboard/agents/cleanup",
            method: "POST",
            result: "success",
            reason: None,
            payload: serde_json::json!({
                "deleted_count": result.rows_affected(),
                "cleanup_days": cleanup_days,
            }),
            request_id: Some(audit_ctx.request_id),
            ip: audit_ctx.ip,
            user_agent: audit_ctx.user_agent,
            before_state: None,
            after_state: Some(serde_json::json!({
                "deleted_count": result.rows_affected(),
                "cleanup_days": cleanup_days,
            })),
        },
    )
    .await
    {
        tracing::error!("audit_log 写入失败(agent.cleanup_offline): {}", e);
    }

    Ok(Json(CleanupResult {
        deleted_count: result.rows_affected(),
    }))
}

// ============================================================================
// Agent Experiences API
// ============================================================================
