//! 配置热重载处理器

use axum::{Json, extract::State, http::StatusCode};
use std::sync::Arc;

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
            state.game_data.update(new_data);

            let reloaded = vec![
                "actions".to_string(),
                "attributes".to_string(),
                "items".to_string(),
                "locations".to_string(),
                "game_rules".to_string(),
                "recipes".to_string(),
                "time".to_string(),
                "narrative".to_string(),
            ];

            Ok(Json(ReloadResponse {
                success: true,
                reloaded,
                timestamp,
                error: None,
                message: Some("Configuration reloaded successfully".to_string()),
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
