//! 配置热重载处理器

use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::StatusCode,
};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

/// 配置重载响应
#[derive(serde::Serialize)]
pub struct ReloadResponse {
    pub success: bool,
    pub reloaded: Vec<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// POST /api/admin/reload-config
pub async fn reload_config_handler(
    State(state): State<Arc<crate::state::AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> Result<Json<ReloadResponse>, (StatusCode, Json<ReloadResponse>)> {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
    let timestamp = chrono::Utc::now();

    // 重新加载配置
    match crate::game_data::load_from_dir(&state.config_dir) {
        Ok(new_data) => {
            // 原子替换缓存
            let attributes_config = new_data.attributes.clone();
            state.game_data.update(new_data);

            // 刷新所有已加载 agent 的 StatusComponent（解决 FINDING-005 5.1）
            let refreshed = refresh_agent_status_metadata(&state, &attributes_config);

            let mut reloaded = vec![
                "actions".to_string(),
                "attributes".to_string(),
                "items".to_string(),
                "locations".to_string(),
                "game_rules".to_string(),
                "recipes".to_string(),
                "time".to_string(),
                "narrative".to_string(),
            ];

            let message = if refreshed > 0 {
                format!(
                    "Configuration reloaded successfully. Refreshed {} agents' StatusComponent metadata.",
                    refreshed
                )
            } else {
                "Configuration reloaded successfully. No active agents to refresh.".to_string()
            };
            reloaded.push(format!("agent_status_metadata({})", refreshed));

            if let Err(e) = crate::db::insert_audit_log(
                &state.db_pool,
                crate::db::AuditLogEntry {
                    event_type: "config.reload",
                    actor_type: "admin",
                    token_type: Some("write"),
                    resource_type: "game_config",
                    resource_id: None,
                    endpoint: "/api/admin/reload-config",
                    method: "POST",
                    result: "success",
                    reason: None,
                    payload: serde_json::json!({
                        "refreshed_agents": refreshed,
                        "reloaded": reloaded.clone(),
                    }),
                    request_id: Some(audit_ctx.request_id),
                    ip: audit_ctx.ip,
                    user_agent: audit_ctx.user_agent,
                    before_state: None,
                    after_state: Some(serde_json::json!({
                        "refreshed_agents": refreshed,
                        "reloaded": reloaded.clone(),
                    })),
                },
            )
            .await
            {
                tracing::error!("audit_log 写入失败(config.reload): {}", e);
            }

            Ok(Json(ReloadResponse {
                success: true,
                reloaded,
                timestamp,
                error: None,
                message: Some(message),
            }))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ReloadResponse {
                success: false,
                reloaded: vec![],
                timestamp,
                error: Some(sanitize_reload_error_message(&e)),
                message: None,
            }),
        )),
    }
}

fn sanitize_reload_error_message<T>(_error: &T) -> String {
    "Internal Server Error".to_string()
}

/// 刷新 DashMap 中所有 agent 的 StatusComponent metadata
///
/// 保留当前值和 max_modifiers，仅更新 decay_per_tick、death_condition 等配置驱动的 metadata。
/// 返回刷新的 agent 数量。
fn refresh_agent_status_metadata(
    state: &Arc<crate::state::AppState>,
    config: &crate::game_data::types::UnifiedAttributesConfig,
) -> usize {
    let mut count = 0;
    for mut entry in state.agent_state_cache.iter_mut() {
        entry.value_mut().status.refresh_from_config(config);
        count += 1;
    }
    if count > 0 {
        info!(
            "Hot-reload: refreshed StatusComponent metadata for {} agents",
            count
        );
    }
    count
}

#[cfg(test)]
mod tests {
    use super::sanitize_reload_error_message;

    #[test]
    fn test_sanitize_reload_error_message_hides_internal_details() {
        let message = sanitize_reload_error_message(&"db exploded");
        assert!(!message.contains("db exploded"));
    }
}
