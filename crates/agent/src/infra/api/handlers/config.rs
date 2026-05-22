// 配置管理 API Handlers
// ============================================================================

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::config::CharacterStatus;

use super::HttpApiState;
use super::character_helpers::{get_active_character, list_characters_from_fs};
use super::dto;

/// 配置信息响应
#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    /// Server HTTP URL
    pub server_http_url: String,
    /// Server WebSocket URL
    pub server_ws_url: String,
    /// 运行模式
    pub runtime_mode: String,
    /// HTTP 端口
    pub port: u16,
}

// ============================================================================

// 服务器配置 API
// ============================================================================

/// 设置服务器地址请求
#[derive(Debug, Deserialize)]
pub struct SetServerRequest {
    /// WebSocket URL
    pub ws_url: String,
    /// HTTP URL（可选）
    pub http_url: Option<String>,
    /// 确认标记（切换服务器需要二次确认）
    /// 当为 true 时执行切换，否则只返回预览信息和警告
    #[serde(default)]
    pub confirm: bool,
}

/// 设置服务器地址响应
#[derive(Debug, Serialize)]
pub struct SetServerResponse {
    pub success: bool,
    pub message: String,
    pub current: ServerConfigResponse,
    /// 是否需要重新注册设备
    pub needs_device_registration: bool,
    /// 是否需要创建新角色
    pub needs_character_creation: bool,
    /// 该服务器上的历史角色
    pub previous_characters: Vec<CharacterSummary>,
    /// 是否需要二次确认（当 confirm=false 且服务器变化时为 true）
    pub requires_confirmation: bool,
    /// 切换警告信息（当 requires_confirmation=true 时返回）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// 角色摘要信息
#[derive(Debug, Serialize)]
pub struct CharacterSummary {
    pub agent_id: String,
    pub name: String,
    pub status: String,
    pub registered_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ServerConfigResponse {
    pub ws_url: String,
    pub http_url: String,
}

/// 获取当前配置
///
/// GET /api/v1/config - 返回当前运行时配置
pub(crate) async fn get_config_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let server_http_url = state.server_http_url.read().await.clone();
    let server_ws_url = state.server_ws_url.read().await.clone();

    Json(ConfigResponse {
        server_http_url,
        server_ws_url,
        runtime_mode: format!("{:?}", state.runtime_mode),
        port: state.actual_port,
    })
}

/// 获取 LLM 停止状态
///
/// GET /api/v1/config/llm-disabled
pub(crate) async fn get_llm_disabled_handler(_state: State<HttpApiState>) -> impl IntoResponse {
    // 从全局标志读取状态
    let disabled = crate::component::llm::direct_client::is_llm_disabled();
    Json(serde_json::json!({"llm_disabled": disabled}))
}

/// 设置 LLM 停止状态
///
/// POST /api/v1/config/llm-disabled
pub(crate) async fn set_llm_disabled_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let disabled = req
        .get("llm_disabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 立即设置全局标志（立即生效）
    crate::component::llm::direct_client::set_llm_disabled(disabled);

    // 异步保存配置到文件
    let config_path = state.config_path.clone();
    let config_disabled = disabled;
    tokio::spawn(async move {
        let mut config = match crate::config::Config::from_file(&config_path) {
            Ok(c) => c,
            Err(e) => {
                error!("[llm-disabled] 读取配置失败: {}", e);
                return;
            }
        };
        config.runtime.llm_disabled = config_disabled;
        if let Err(e) = config.save_to_file(&config_path) {
            error!("[llm-disabled] 保存配置失败: {}", e);
        }
    });

    if disabled {
        tracing::warn!("[llm-disabled] LLM 调用已停止");
    } else {
        tracing::info!("[llm-disabled] LLM 调用已恢复");
    }

    Json(serde_json::json!({
        "success": true,
        "llm_disabled": disabled,
        "message": if disabled { "LLM 调用已停止" } else { "LLM 调用已恢复" }
    }))
    .into_response()
}

/// 获取自动重生开关状态
///
/// GET /api/v1/config/auto-rebirth
pub(crate) async fn get_auto_rebirth_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let enabled = state
        .auto_rebirth
        .load(std::sync::atomic::Ordering::Relaxed);
    Json(serde_json::json!({"auto_rebirth": enabled}))
}

/// 设置自动重生开关
///
/// POST /api/v1/config/auto-rebirth
pub(crate) async fn set_auto_rebirth_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let enabled = req
        .get("auto_rebirth")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    state
        .auto_rebirth
        .store(enabled, std::sync::atomic::Ordering::Relaxed);

    let config_path = state.config_path.clone();
    let config_enabled = enabled;
    tokio::spawn(async move {
        let mut config = match crate::config::Config::from_file(&config_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("[auto-rebirth] 读取配置失败: {}", e);
                return;
            }
        };
        config.runtime.auto_rebirth = config_enabled;
        if let Err(e) = config.save_to_file(&config_path) {
            tracing::error!("[auto-rebirth] 保存配置失败: {}", e);
        }
    });

    if enabled {
        tracing::info!("[auto-rebirth] 自动重生已开启");
    } else {
        tracing::warn!("[auto-rebirth] 自动重生已关闭");
    }

    Json(serde_json::json!({
        "success": true,
        "auto_rebirth": enabled,
    }))
    .into_response()
}

/// 获取动作类型到中文描述的映射
///
/// GET /api/v1/actions - 返回 action_type -> name 映射（短中文名，用于前端展示）
pub(crate) async fn get_actions_handler() -> impl IntoResponse {
    let actions = crate::infra::api::cognitive_context::load_available_actions_from_file();
    let map: std::collections::HashMap<String, String> =
        actions.into_iter().map(|a| (a.action, a.name)).collect();
    Json(map)
}

/// GET /api/v1/setup/status - 返回引导状态
pub(crate) async fn setup_status_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(_) => {
            return Json(dto::SetupStatusResponse {
                needs_setup: true,
                has_server: false,
                has_llm: false,
                has_character: false,
                current_character: None,
                is_dead: false,
                actual_port: state.actual_port,
            })
            .into_response();
        }
    };

    let has_server = !config.server.ws_url.is_empty();
    let has_llm = config.llm.model.is_some() || config.llm.base_url.is_some();

    // is_dead=true 表示角色已死亡/等待转生，即使文件系统仍为 Alive
    let is_dead = state.is_dead.load(std::sync::atomic::Ordering::Relaxed);

    // 从文件系统检查是否有活跃角色（is_dead 时视为无活跃角色）
    let (has_character, current_character) = if is_dead {
        (false, None)
    } else {
        match get_active_character(&state).await {
            Ok(Some(c)) => (true, Some(c.name.clone())),
            _ => (false, None),
        }
    };

    let needs_setup = !has_server || !has_llm;

    Json(dto::SetupStatusResponse {
        needs_setup,
        has_server,
        has_llm,
        has_character,
        current_character,
        is_dead,
        actual_port: state.actual_port,
    })
    .into_response()
}

/// 配置重载请求
#[derive(Debug, Deserialize)]
pub struct ConfigReloadRequest {
    /// 新的 Server HTTP URL（可选）
    pub server_http_url: Option<String>,
    /// 新的 Server WebSocket URL（可选）
    pub server_ws_url: Option<String>,
}

/// 配置重载响应
#[derive(Debug, Serialize)]
pub struct ConfigReloadResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
    /// 更新后的配置
    pub config: Option<ConfigResponse>,
}

/// 热重载配置
///
/// POST /api/v1/config/reload - 热重载服务器配置
///
/// 支持两种方式：
/// 1. 不传参数：从配置文件重新加载
/// 2. 传入参数：直接更新配置（不写文件）
pub(crate) async fn reload_config_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<ConfigReloadRequest>,
) -> impl IntoResponse {
    use tracing::info;

    // 如果提供了参数，直接更新
    if req.server_http_url.is_some() || req.server_ws_url.is_some() {
        if let Some(http_url) = &req.server_http_url {
            let mut url = state.server_http_url.write().await;
            *url = http_url.clone();
            info!("[config] 已更新 server_http_url: {}", http_url);
        }
        if let Some(ws_url) = &req.server_ws_url {
            let mut url = state.server_ws_url.write().await;
            *url = ws_url.clone();
            info!("[config] 已更新 server_ws_url: {}", ws_url);
        }

        let config = ConfigResponse {
            server_http_url: state.server_http_url.read().await.clone(),
            server_ws_url: state.server_ws_url.read().await.clone(),
            runtime_mode: format!("{:?}", state.runtime_mode),
            port: state.actual_port,
        };

        return Json(ConfigReloadResponse {
            success: true,
            message: "配置已更新".to_string(),
            config: Some(config),
        })
        .into_response();
    }

    // 没有参数，从配置文件重新加载
    let config_path = state.config_path.clone();
    match crate::config::Config::from_file(&config_path) {
        Ok(config) => {
            // Fail Fast: 校验 EarthSoul 配置
            if let Err(e) = config.earth_soul.validate() {
                tracing::error!("[config] earth_soul 配置校验失败: {}（保留旧配置）", e);
                return Json(ConfigReloadResponse {
                    success: false,
                    message: format!("earth_soul 配置校验失败: {}", e),
                    config: None,
                })
                .into_response();
            }

            // 更新 server URLs
            {
                let mut http_url = state.server_http_url.write().await;
                *http_url = config.server.http_url.clone();
            }
            {
                let mut ws_url = state.server_ws_url.write().await;
                *ws_url = config.server.ws_url.clone();
            }

            info!(
                "[config] 已从文件重载配置: http={}, ws={}",
                config.server.http_url, config.server.ws_url
            );

            // 重建 LLM Client（clone Arc 后立即释放读锁）
            let container = {
                let guard = state.llm_container.read().await;
                guard.clone()
            };
            if let Some(container) = container {
                match crate::component::llm::build_fallback_client(
                    &config.llm,
                    config.llm.enable_streaming,
                    Some(config.earth_soul.clone()),
                ) {
                    Ok(new_client) => {
                        *container.write().await = new_client;
                        info!(
                            "[config] LLM Client 已重建: provider={}, model={:?}",
                            config.llm.provider, config.llm.model
                        );
                    }
                    Err(e) => {
                        tracing::error!("[config] LLM Client 重建失败: {}（保留旧实例）", e);
                    }
                }
            }

            let response_config = ConfigResponse {
                server_http_url: config.server.http_url,
                server_ws_url: config.server.ws_url,
                runtime_mode: format!("{:?}", state.runtime_mode),
                port: state.actual_port,
            };

            Json(ConfigReloadResponse {
                success: true,
                message: "配置已从文件重载".to_string(),
                config: Some(response_config),
            })
            .into_response()
        }
        Err(e) => {
            error!("[config] 重载配置失败: {}", e);
            Json(ConfigReloadResponse {
                success: false,
                message: format!("重载配置失败: {}", e),
                config: None,
            })
            .into_response()
        }
    }
}

/// 保存服务器配置到文件（使用类型化 Config，避免 serde_yaml::Value 破坏其他字段）
///
/// # Arguments
/// * `config_path` - 配置文件完整路径
/// * `ws_url` - WebSocket URL
/// * `http_url` - HTTP URL（可选）
fn save_server_config(
    config_path: &std::path::PathBuf,
    ws_url: &str,
    http_url: Option<&str>,
) -> anyhow::Result<()> {
    let mut config = if config_path.exists() {
        crate::config::Config::from_file(config_path)?
    } else {
        crate::config::Config::default()
    };

    config.server.ws_url = ws_url.to_string();
    if let Some(http) = http_url {
        config.server.http_url = http.to_string();
    }

    config.save_to_file(config_path)?;
    Ok(())
}

/// POST /api/v1/config/server - 设置服务器地址
///
/// 设置服务器地址并触发重连。
/// 配置会持久化到 `config_path` 文件。
///
/// 服务器切换需要二次确认（防止误操作）：
/// 1. 第一次调用（confirm=false）返回预览信息和警告
/// 2. 第二次调用（confirm=true）执行切换
pub(crate) async fn set_server_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<SetServerRequest>,
) -> impl IntoResponse {
    // 获取旧的服务器地址
    let old_ws_url = state.server_ws_url.read().await.clone();

    // 计算新的 http_url
    let http_url_value = req
        .http_url
        .clone()
        .unwrap_or_else(|| crate::config::ws_to_http_url(&req.ws_url));

    // 检查是否切换到了不同的服务器
    let server_changed = old_ws_url != req.ws_url;

    // 预计算新服务器目录（server_changed 时两处共用）
    let new_server_dir = state
        .server_dir
        .read()
        .await
        .parent()
        .unwrap_or(state.server_dir.read().await.as_path())
        .to_path_buf()
        .join(crate::config::server_key(&req.ws_url));
    let new_character_dir = new_server_dir.join("characters");

    let mut needs_device_registration = false;
    let mut needs_character_creation = false;
    let mut previous_characters: Vec<CharacterSummary> = vec![];

    // 如果服务器地址变化，检查设备注册状态和角色状态
    if server_changed {
        info!(
            "[config] 检测到服务器地址变更: {} -> {}",
            old_ws_url, req.ws_url
        );

        // 服务器变更后，设备需要重新注册（每个服务器有独立的设备表）
        needs_device_registration = true;

        // 检查是否有该服务器上的存活角色
        match list_characters_from_fs(&new_character_dir) {
            Ok(all_characters) => {
                // 过滤出该服务器的角色
                let server_characters: Vec<_> = all_characters
                    .iter()
                    .filter(|c| {
                        c.server_url
                            .as_ref()
                            .map(|u| u == &http_url_value)
                            .unwrap_or(false)
                    })
                    .collect();

                previous_characters = server_characters
                    .iter()
                    .map(|c| CharacterSummary {
                        agent_id: c.agent_id.map(|id| id.to_string()).unwrap_or_default(),
                        name: c.name.clone(),
                        status: match c.status {
                            CharacterStatus::Alive => "alive".to_string(),
                            CharacterStatus::Dead => "dead".to_string(),
                            CharacterStatus::Retired => "retired".to_string(),
                        },
                        registered_at: c.registered_at.map(|t| t.to_rfc3339()),
                    })
                    .collect();

                // 检查是否有存活角色
                let has_alive = server_characters
                    .iter()
                    .any(|c| c.status == CharacterStatus::Alive);
                if !has_alive {
                    needs_character_creation = true;
                }
            }
            Err(e) => {
                error!("读取角色列表失败: {}", e);
                needs_character_creation = true;
            }
        }
    }

    // 如果服务器变化但未确认，返回预览和警告（不执行）
    if server_changed && !req.confirm {
        let warning = format!(
            "切换服务器 {} -> {} 需要二次确认。当前角色状态将被保留，但需要重新连接。",
            old_ws_url, req.ws_url
        );
        return (
            StatusCode::OK,
            Json(SetServerResponse {
                success: true,
                message: "请确认服务器切换".to_string(),
                current: ServerConfigResponse {
                    ws_url: old_ws_url,
                    http_url: state.server_http_url.read().await.clone(),
                },
                needs_device_registration,
                needs_character_creation,
                previous_characters,
                requires_confirmation: true,
                warning: Some(warning),
            }),
        );
    }

    // 1. 更新内存中的配置
    {
        let mut ws_url = state.server_ws_url.write().await;
        *ws_url = req.ws_url.clone();
    }
    {
        let mut url = state.server_http_url.write().await;
        *url = http_url_value.clone();
    }

    // 1.5 更新 server_dir 和 character_dir（服务器切换后指向新目录）
    if server_changed {
        *state.server_dir.write().await = new_server_dir;
        *state.character_dir.write().await = new_character_dir;
        info!(
            "[config] server_dir 更新为: {}",
            state.server_dir.read().await.display()
        );
    }

    // 2. 持久化到配置文件（config_path 是文件路径，非目录）
    if let Err(e) = save_server_config(&state.config_path, &req.ws_url, Some(&http_url_value)) {
        error!("保存配置失败: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SetServerResponse {
                success: false,
                message: format!("保存配置失败: {}", e),
                current: ServerConfigResponse {
                    ws_url: req.ws_url,
                    http_url: http_url_value,
                },
                needs_device_registration: false,
                needs_character_creation: false,
                previous_characters: vec![],
                requires_confirmation: false,
                warning: None,
            }),
        );
    }

    // 3. 触发重连（通过 channel 通知主循环）
    if let Some(ref tx) = state.reconnect_tx {
        let reconnect_req = crate::infra::api::ReconnectRequest {
            ws_url: req.ws_url.clone(),
            agent_id: None,
        };
        if let Err(e) = tx.send(reconnect_req) {
            error!("发送重连请求失败: {}", e);
        } else {
            info!("[config] 触发 WebSocket 重连: {}", req.ws_url);
        }
    }

    // 构建响应消息
    let message = if needs_device_registration {
        "服务器地址已更新，需要重新注册设备".to_string()
    } else if needs_character_creation {
        "服务器地址已更新，需要创建新角色".to_string()
    } else {
        "服务器地址已更新，正在重连...".to_string()
    };

    let response = SetServerResponse {
        success: true,
        message,
        current: ServerConfigResponse {
            ws_url: req.ws_url,
            http_url: http_url_value,
        },
        needs_device_registration,
        needs_character_creation,
        previous_characters,
        requires_confirmation: false,
        warning: None,
    };

    (StatusCode::OK, Json(response))
}

// ============================================================================
