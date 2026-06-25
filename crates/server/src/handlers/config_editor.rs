use crate::game_data::loaders::config_format::{ConfigFormat, parse_config};
use crate::game_data::types::{
    UnifiedActionsConfig, UnifiedAttributesConfig, UnifiedGameRulesConfig,
    UnifiedInitialInventoryConfig, UnifiedInventoryConfig, UnifiedItemsConfig,
    UnifiedLocationsConfig, UnifiedNetworkConfig, UnifiedWorldBuildingRulesConfig,
};
use crate::state::AppState;
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

/// 获取配置目录路径
fn get_config_dir() -> PathBuf {
    crate::paths::get_config_dir()
}

/// 读取原文件内容用于备份 + 审计。
///
/// 严格语义：
/// - 文件不存在 → `Ok(None)`，允许覆盖创建（首次写配置）
/// - 文件存在但读失败（权限、IO 错误）→ `Err`，禁止覆盖，避免后续回滚失去依据
/// - 读取成功 → `Ok(Some(content))`
///
/// 之前用 `fs::read_to_string(&path).ok()` 会把所有失败折叠为 None，
/// 触发"读不到原文还继续写"的双重故障。
fn read_original_content(path: &std::path::Path) -> std::io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
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

/// 支持编辑的配置文件名集合（与 validate_config_content 中的已知类型一致）
fn is_known_config(filename: &str) -> bool {
    matches!(
        filename,
        "game_rules.json"
            | "game_rules.yaml"
            | "game_rules.yml"
            | "items.json"
            | "items.yaml"
            | "items.yml"
            | "actions.json"
            | "actions.yaml"
            | "actions.yml"
            | "initial_inventory.json"
            | "initial_inventory.yaml"
            | "initial_inventory.yml"
            | "inventory.json"
            | "inventory.yaml"
            | "inventory.yml"
            | "network.json"
            | "network.yaml"
            | "network.yml"
            | "locations.json"
            | "locations.yaml"
            | "locations.yml"
            | "attributes.json"
            | "attributes.yaml"
            | "attributes.yml"
            | "world_building_rules.json"
            | "world_building_rules.yaml"
            | "world_building_rules.yml"
            | "time.json"
            | "time.yaml"
            | "time.yml"
            | "recipes.json"
            | "recipes.yaml"
            | "recipes.yml"
            | "narrative.json"
            | "narrative.yaml"
            | "narrative.yml"
            | "souls.json"
            | "souls.yaml"
            | "souls.yml"
            | "action_evolution.json"
            | "action_evolution.yaml"
            | "action_evolution.yml"
    )
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
                && get_config_format(name).is_some()
                && is_known_config(name)
            {
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

    // 只允许读取已知类型的配置文件
    if !is_known_config(&filename) {
        return Err(StatusCode::FORBIDDEN);
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Path(filename): Path<String>,
    Json(payload): Json<UpdateConfigRequest>,
) -> impl IntoResponse {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
    if let Err(code) = validate_filename(&filename) {
        return (code, "Invalid filename".to_string());
    }

    // 阶段1: 校验配置格式
    if let Some(format) = get_config_format(&filename)
        && let Err(e) = validate_config_content(&filename, &payload.content, format)
    {
        return (StatusCode::BAD_REQUEST, format!("Validation failed: {}", e));
    }

    let path = get_config_dir().join(&filename);

    // 阶段2: 备份原文件内容（用于回滚）
    // 严格区分"不存在（首次写）"与"存在但读失败"；后者必须阻断写入
    let original_content = match read_original_content(&path) {
        Ok(content) => content,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read original config for backup: {}", e),
            );
        }
    };

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
            if let Err(e) = crate::db::insert_audit_log(
                &state.db_pool,
                crate::db::AuditLogEntry {
                    event_type: "config.update",
                    actor_type: "admin",
                    token_type: Some("write"),
                    resource_type: "config_file",
                    resource_id: Some(filename.clone()),
                    endpoint: "/api/config/{filename}",
                    method: "PUT",
                    result: "success",
                    reason: None,
                    payload: serde_json::json!({
                        "filename": filename,
                        "content_length": payload.content.len(),
                    }),
                    request_id: Some(audit_ctx.request_id),
                    ip: audit_ctx.ip,
                    user_agent: audit_ctx.user_agent,
                    before_state: original_content
                        .as_ref()
                        .map(|content| serde_json::json!({"content_length": content.len()})),
                    after_state: Some(serde_json::json!({
                        "content_length": payload.content.len(),
                    })),
                },
            )
            .await
            {
                error!("audit_log 写入失败(config.update): {}", e);
            }
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
                        "Hot reload failed and rollback also failed. Please check server logs for details.".to_string()
                    );
                }
                info!("Config file rolled back successfully");
            }

            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Hot reload failed, file rolled back. Please check server logs for details.".to_string(),
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
    // 只允许编辑已知类型的配置文件（防止误编辑其他 yaml/json 文件）
    if !is_known_config(filename) {
        return Err(StatusCode::FORBIDDEN);
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
        "world_building_rules.json" | "world_building_rules.yaml" | "world_building_rules.yml" => {
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

#[cfg(test)]
mod tests {
    use super::read_original_content;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "config_editor_test_{suffix}_{}",
            uuid::Uuid::new_v4()
        ))
    }

    /// 验证 P1-8：文件不存在时返回 Ok(None)，允许覆盖创建（首次写配置）。
    #[test]
    fn test_read_original_content_returns_none_for_missing_file() {
        let dir = temp_dir("missing");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("does_not_exist.yaml");

        let result = read_original_content(&path).expect("NotFound must not be Err");
        assert!(result.is_none());

        fs::remove_dir_all(&dir).ok();
    }

    /// 验证 P1-8：文件存在但读失败时必须 Err，禁止静默回退为 None。
    /// 这里用"目录占位文件路径"模拟：把目录当成文件读，
    /// 任何非 NotFound 错误都必须冒泡，让 caller 阻断后续覆盖。
    #[test]
    fn test_read_original_content_propagates_non_notfound_error() {
        let dir = temp_dir("dir_as_file");
        fs::create_dir_all(&dir).unwrap();

        let result = read_original_content(&dir);
        assert!(
            result.is_err(),
            "reading a directory as a file must NOT silently return Ok(None); \
             otherwise the hot-reload rollback path loses its original content"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// 验证 P1-8：文件存在且可读时返回 Ok(Some(content))。
    #[test]
    fn test_read_original_content_returns_existing_content() {
        let dir = temp_dir("existing");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("actions.yaml");
        fs::write(&path, "version: '2.0'\n").unwrap();

        let result = read_original_content(&path).expect("read should succeed");
        assert_eq!(result.as_deref(), Some("version: '2.0'\n"));

        fs::remove_dir_all(&dir).ok();
    }
}
