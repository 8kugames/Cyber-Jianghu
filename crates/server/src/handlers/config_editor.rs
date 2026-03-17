use crate::game_data::types::{
    UnifiedActionsConfig, UnifiedGameRulesConfig, UnifiedInitialInventoryConfig,
    UnifiedInventoryConfig, UnifiedItemsConfig, UnifiedLocationsConfig, UnifiedNetworkConfig,
    UnifiedAttributesConfig,
};
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use cyber_jianghu_protocol::WorldBuildingRules;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

/// 获取配置目录路径
fn get_config_dir() -> PathBuf {
    crate::paths::get_config_dir()
}

#[derive(Serialize)]
pub struct ConfigFile {
    pub name: String,
    pub size: u64,
}

pub async fn list_configs() -> Json<Vec<ConfigFile>> {
    let config_dir = get_config_dir();
    let mut files = Vec::new();

    if let Ok(entries) = fs::read_dir(config_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_file() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.ends_with(".json")
                            || name.ends_with(".yaml")
                            || name.ends_with(".yml")
                        {
                            files.push(ConfigFile {
                                name: name.to_string(),
                                size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                            });
                        }
                    }
                }
            }
        }
    }

    // Sort by name
    files.sort_by(|a, b| a.name.cmp(&b.name));

    Json(files)
}

#[derive(Serialize)]
pub struct ConfigContent {
    pub content: String,
}

pub async fn get_config_content(
    Path(filename): Path<String>,
) -> Result<Json<ConfigContent>, StatusCode> {
    if let Err(_) = validate_filename(&filename) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let path = get_config_dir().join(&filename);
    match fs::read_to_string(&path) {
        Ok(content) => Ok(Json(ConfigContent { content })),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

#[derive(Deserialize)]
pub struct UpdateConfigRequest {
    pub content: String,
}

pub async fn update_config_content(
    State(state): State<Arc<AppState>>,
    Path(filename): Path<String>,
    Json(payload): Json<UpdateConfigRequest>,
) -> impl IntoResponse {
    if let Err(code) = validate_filename(&filename) {
        return (code, "Invalid filename".to_string());
    }

    // 阶段1: 校验 JSON 格式
    if filename.ends_with(".json") {
        if let Err(e) = validate_json_content(&filename, &payload.content) {
            return (StatusCode::BAD_REQUEST, format!("Validation failed: {}", e));
        }
    }

    let path = get_config_dir().join(&filename);

    // 阶段2: 备份原文件内容（用于回滚）
    let original_content = fs::read_to_string(&path).ok();

    // 阶段3: 写入新内容
    if let Err(e) = fs::write(&path, &payload.content) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write file: {}", e),
        );
    }

    // 阶段4: 尝试热更新（加载并更新内存）
    info!("Config file updated: {}, attempting hot reload...", filename);
    match crate::game_data::load_from_dir(get_config_dir()) {
        Ok(new_data) => {
            // 加载成功，更新内存
            state.game_data.update(new_data);
            info!("Game configuration hot-reloaded successfully");
            (
                StatusCode::OK,
                "Config saved and hot-reloaded successfully".to_string(),
            )
        }
        Err(e) => {
            // 阶段5: 加载失败，回滚文件
            error!("Hot reload failed: {}, rolling back file...", e);
            
            if let Some(original) = original_content {
                if let Err(rollback_err) = fs::write(&path, original) {
                    error!("Rollback also failed: {}", rollback_err);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Hot reload failed and rollback also failed: {} | {}", e, rollback_err),
                    );
                }
                info!("Config file rolled back successfully");
            }
            
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Hot reload failed, file rolled back: {}", e),
            )
        }
    }
}

fn validate_filename(filename: &str) -> Result<(), StatusCode> {
    if filename.contains("..") || filename.contains("/") || filename.contains("\\") {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !filename.ends_with(".json") && !filename.ends_with(".yaml") && !filename.ends_with(".yml") {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

fn validate_json_content(filename: &str, content: &str) -> Result<(), String> {
    // Helper macro to validate specific type
    macro_rules! validate {
        ($type:ty) => {
            serde_json::from_str::<$type>(content)
                .map_err(|e: serde_json::Error| e.to_string())
                .map(|_| ())
        };
    }

    match filename {
        "game_rules.json" => validate!(UnifiedGameRulesConfig),
        "items.json" => validate!(UnifiedItemsConfig),
        "actions.json" => validate!(UnifiedActionsConfig),
        "initial_inventory.json" => validate!(UnifiedInitialInventoryConfig),
        "inventory.json" => validate!(UnifiedInventoryConfig),
        "network.json" => validate!(UnifiedNetworkConfig),
        "locations.json" => validate!(UnifiedLocationsConfig),
        "attributes.json" => validate!(UnifiedAttributesConfig),
        "world-building-rules.json" => validate!(WorldBuildingRules),
        _ => {
            // For unknown JSON files, just check if it's valid JSON
            serde_json::from_str::<serde_json::Value>(content)
                .map_err(|e| e.to_string())
                .map(|_| ())
        }
    }
}
