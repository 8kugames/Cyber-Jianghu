//! 配置热重载处理器

use axum::{Json, extract::State, http::StatusCode};
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
) -> Result<Json<ReloadResponse>, (StatusCode, Json<ReloadResponse>)> {
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
                error: Some(format!("{:?}", e)),
                message: None,
            }),
        )),
    }
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
