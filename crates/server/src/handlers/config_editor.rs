use crate::game_data::loaders::config_format::{ConfigFormat, parse_config};
use crate::game_data::types::{
    UnifiedActionsConfig, UnifiedAttributesConfig, UnifiedGameRulesConfig,
    UnifiedInitialInventoryConfig, UnifiedInventoryConfig, UnifiedItemsConfig,
    UnifiedLocationsConfig, UnifiedNetworkConfig, UnifiedWorldBuildingRulesConfig,
};
use crate::state::AppState;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

/// 获取配置目录路径
fn get_config_dir() -> PathBuf {
    crate::paths::get_config_dir()
}

/// 从文件名推断配置格式
fn get_config_format(filename: &str) -> Option<ConfigFormat> {
    if filename.ends_with(".json") {
        Some(ConfigFormat::Json)
    } else if filename.ends_with(".yaml") || filename.ends_with(".yml") {
        Some(ConfigFormat::Yaml)
    } else {
        None
    }
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
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && get_config_format(name).is_some() {
                        files.push(ConfigFile {
                            name: name.to_string(),
                            size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                        });
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
    if validate_filename(&filename).is_err() {
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

    // 阶段1: 校验配置格式
    if let Some(format) = get_config_format(&filename)
        && let Err(e) = validate_config_content(&filename, &payload.content, format) {
            return (StatusCode::BAD_REQUEST, format!("Validation failed: {}", e));
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
    info!(
        "Config file updated: {}, attempting hot reload...",
        filename
    );
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
                        format!(
                            "Hot reload failed and rollback also failed: {} | {}",
                            e, rollback_err
                        ),
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
    if get_config_format(filename).is_none() {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

/// 验证配置内容
fn validate_config_content(
    filename: &str,
    content: &str,
    format: ConfigFormat,
) -> Result<(), String> {
    // Helper macro to validate specific type
    macro_rules! validate {
        ($type:ty) => {
            parse_config::<$type>(content, format)
                .map_err(|e| e.to_string())
                .map(|_| ())
        };
    }

    // 支持 JSON 和 YAML 文件名的验证
    match filename {
        // JSON 文件名
        "game_rules.json" | "game_rules.yaml" | "game_rules.yml" => {
            validate!(UnifiedGameRulesConfig)
        }
        "items.json" | "items.yaml" | "items.yml" => validate!(UnifiedItemsConfig),
        "actions.json" | "actions.yaml" | "actions.yml" => validate!(UnifiedActionsConfig),
        "initial_inventory.json" | "initial_inventory.yaml" | "initial_inventory.yml" => {
            validate!(UnifiedInitialInventoryConfig)
        }
        "inventory.json" | "inventory.yaml" | "inventory.yml" => validate!(UnifiedInventoryConfig),
        "network.json" | "network.yaml" | "network.yml" => validate!(UnifiedNetworkConfig),
        "locations.json" | "locations.yaml" | "locations.yml" => validate!(UnifiedLocationsConfig),
        "attributes.json" | "attributes.yaml" | "attributes.yml" => {
            validate!(UnifiedAttributesConfig)
        }
        "world-building-rules.json" | "world-building-rules.yaml" | "world-building-rules.yml" => {
            validate!(UnifiedWorldBuildingRulesConfig)
        }
        "time.json" | "time.yaml" | "time.yml" => {
            validate!(crate::game_data::types::UnifiedTimeConfig)
        }
        "recipes.json" | "recipes.yaml" | "recipes.yml" => {
            validate!(crate::game_data::types::UnifiedRecipesConfig)
        }
        "narrative.json" | "narrative.yaml" | "narrative.yml" => {
            validate!(crate::game_data::types::UnifiedNarrativeConfig)
        }
        _ => {
            // For unknown files, just check if it's valid in the given format
            parse_config::<serde_json::Value>(content, format)
                .map_err(|e| e.to_string())
                .map(|_| ())
        }
    }
}
