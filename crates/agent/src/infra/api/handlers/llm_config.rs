// LLM 配置 API Handlers
// ============================================================================

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Serialize;
use tracing::{error, info};

use super::HttpApiState;
use super::basic::ErrorResponse;
use super::dto;

/// GET /api/v1/config/llm/providers - 返回支持的 LLM Provider 列表
///
/// 从 LlmProvider 枚举自动派生，新增 Provider 时无需手动维护此列表。
/// OpenClaw 特殊处理：检查配置文件是否存在，不存在则禁选。
pub(crate) async fn get_llm_providers_handler() -> impl IntoResponse {
    use crate::component::llm::LlmProvider;

    let openclaw_config_path = crate::component::llm::direct_client::OpenClawConfig::config_path();
    let has_openclaw_config = openclaw_config_path
        .as_ref()
        .is_ok_and(|path| path.exists());

    let providers: Vec<dto::LlmProviderInfo> = LlmProvider::ALL
        .iter()
        .map(|p| {
            let (disabled, disabled_reason) = if matches!(p, LlmProvider::OpenClaw) {
                (
                    Some(!has_openclaw_config),
                    if !has_openclaw_config {
                        Some("OpenClaw 不存在".to_string())
                    } else {
                        None
                    },
                )
            } else {
                (None, None)
            };
            dto::LlmProviderInfo {
                value: p.as_str().to_string(),
                label: p.display_label().to_string(),
                requires_base_url: p.requires_base_url(),
                disabled,
                disabled_reason,
            }
        })
        .collect();

    Json(dto::LlmProvidersResponse { providers })
}

/// GET /api/v1/config/llm/providers/openclaw/defaults - 返回 OpenClaw 默认配置
///
/// **仅当用户选择 openclaw provider 时调用此接口**
/// 读取 `~/.openclaw/openclaw.json` 获取 gateway_url
/// 注意：不读取 api_key，api_key 必须由用户手动输入
pub(crate) async fn get_openclaw_defaults_handler() -> impl IntoResponse {
    use crate::component::llm::direct_client::OpenClawConfig;

    match OpenClawConfig::load() {
        Ok(config) => {
            let base_url = config.gateway_url().map(|s| s.to_string());
            Json(dto::OpenClawDefaultsResponse {
                base_url,
                model: None, // OpenClaw 配置中没有默认模型
            })
        }
        Err(e) => {
            tracing::warn!("Failed to load OpenClaw config: {}", e);
            Json(dto::OpenClawDefaultsResponse {
                base_url: None,
                model: None,
            })
        }
    }
}

fn llm_config_to_info(c: &crate::config::LlmConfig) -> dto::LlmConfigInfo {
    dto::LlmConfigInfo {
        provider: c.provider.clone(),
        model: c.model.clone().unwrap_or_default(),
        base_url: c.base_url.clone(),
        has_api_key: c.api_key.as_ref().is_some_and(|k| !k.is_empty()),
        temperature: c.temperature,
        max_tokens: c.max_tokens,
        context_window_tokens: c.context_window_tokens,
        enable_streaming: c.enable_streaming,
        enable_thinking: c.enable_thinking,
        summary_trigger_ratio: c.summary_trigger_ratio,
        keep_recent_turns: c.keep_recent_turns,
        idle_rotate_threshold: c.idle_rotate_threshold,
        fallback_models: c.fallback_models.clone(),
    }
}

fn apply_llm_update(target: &mut crate::config::LlmConfig, update: &dto::LlmConfigUpdateDetails) {
    target.provider = update.provider.clone();
    target.base_url = update.base_url.clone();
    target.api_key = if update.api_key.is_empty() {
        None
    } else {
        Some(update.api_key.clone())
    };
    target.model = Some(update.model.clone());
    if let Some(v) = update.temperature {
        target.temperature = v;
    }
    if let Some(v) = update.max_tokens {
        target.max_tokens = v;
    }
    if let Some(v) = update.context_window_tokens {
        target.context_window_tokens = v;
    }
    if let Some(v) = update.enable_streaming {
        target.enable_streaming = v;
    }
    if let Some(v) = update.enable_thinking {
        target.enable_thinking = Some(v);
    }
    if let Some(v) = update.summary_trigger_ratio {
        target.summary_trigger_ratio = v;
    }
    if let Some(v) = update.keep_recent_turns {
        target.keep_recent_turns = v;
    }
    if let Some(v) = update.idle_rotate_threshold {
        target.idle_rotate_threshold = v;
    }
    if let Some(ref v) = update.fallback_models {
        target.fallback_models = v.clone();
    }
}

/// GET /api/v1/config/llm - 返回当前 LLM 配置
pub(crate) async fn get_llm_config_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("[llm] 读取配置文件失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "config_read_error".to_string(),
                    message: format!("读取配置文件失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    let actor = llm_config_to_info(&config.llm);
    let reflector = config.llm_reflector.as_ref().map(llm_config_to_info);

    let response = dto::LlmConfigResponse {
        actor,
        reflector,
        reflector_inherits_actor: config.llm_reflector.is_none(),
        runtime_mode: state.runtime_mode.to_string(),
        llm_secondary: config.llm_secondary.as_ref().map(llm_config_to_info),
        scenario_routing: effective_scenario_routing(&config.llm),
        scenario_keys: scenario_keys_json(),
    };

    Json(response).into_response()
}

/// 可配置场景键白名单（GET/POST 响应共用；单一事实源 scenario.rs）
fn scenario_keys_json() -> Vec<String> {
    crate::component::llm::scenario::CONFIGURABLE_SCENARIO_KEYS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 生效场景路由：显式配置优先，未配置键按内置默认（轻量场景走从），
/// 输出覆盖全部可配置键的全量视图（面板直接渲染真相）
fn effective_scenario_routing(
    llm: &crate::config::LlmConfig,
) -> std::collections::HashMap<String, crate::config::ScenarioRouteConfig> {
    use crate::component::llm::scenario::{CONFIGURABLE_SCENARIO_KEYS, defaults_to_secondary};
    CONFIGURABLE_SCENARIO_KEYS
        .iter()
        .map(|k| {
            let route = llm.scenario_routing.get(*k).cloned().unwrap_or(
                crate::config::ScenarioRouteConfig {
                    via: if defaults_to_secondary(k) {
                        crate::config::ScenarioVia::Secondary
                    } else {
                        crate::config::ScenarioVia::Primary
                    },
                    max_tokens: None,
                },
            );
            (k.to_string(), route)
        })
        .collect()
}

/// LLM 配置更新响应
#[derive(Debug, Serialize)]
pub struct LlmConfigUpdateResponse {
    pub success: bool,
    pub message: String,
    pub config: Option<dto::LlmConfigResponse>,
}

/// 解析有效 API Key。
///
/// 前端 GET /api/v1/config/llm 出于安全只返回 `has_api_key: bool`，
/// 故密钥输入框默认为空。当用户未重新输入（空串）时，应复用已保存密钥，
/// 而非当作"清空密钥"——对齐 Server 端 config_llm.rs:173-177 的兜底语义。
///
/// - `req_key` trim 后非空 → 使用新值
/// - `req_key` 为空 → 回退到 `saved`（trim 后非空才用）
/// - 两者皆空 → 返回空串（下游既有"空串→None"语义保持不变）
fn resolve_api_key(req_key: &str, saved: Option<&str>) -> String {
    let req_trimmed = req_key.trim();
    if !req_trimmed.is_empty() {
        return req_trimmed.to_string();
    }
    saved
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_default()
}

/// 验证 LLM 配置并创建测试客户端
fn validate_llm_config(
    provider: &str,
    model: &str,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> anyhow::Result<()> {
    use crate::component::llm::LlmProvider;

    // 验证 provider（通过 enum parse 代替硬编码字符串列表）
    let parsed = LlmProvider::parse(provider)
        .ok_or_else(|| anyhow::anyhow!("不支持的 Provider: {}", provider))?;

    // 验证 model
    if model.is_empty() {
        anyhow::bail!("model 不能为空");
    }

    // 验证 API Key 非空（仅提示，不强制格式）
    if let Some(key) = api_key
        && key.is_empty()
    {
        anyhow::bail!("api_key 不能为空字符串");
    }

    // 检查 requires_base_url 的 provider 是否提供了 base_url
    if parsed.requires_base_url() && (base_url.is_none() || base_url.is_none_or(|u| u.is_empty())) {
        anyhow::bail!("{} 需要提供 base_url", provider);
    }

    Ok(())
}

/// POST /api/v1/config/llm - 更新 LLM 配置
///
/// 验证配置、测试 LLM 连接、保存配置文件
pub(crate) async fn update_llm_config_handler(
    State(state): State<HttpApiState>,
    Json(mut req): Json<dto::LlmConfigUpdate>,
) -> impl IntoResponse {
    use crate::component::llm::{DirectLlmClient, DirectLlmClientConfig, LlmClient, LlmProvider};

    // 0. 归一化 api_key：前端 GET 不回显密钥（仅 has_api_key: bool），
    //    故空串表示"用户未修改"，需回退到已保存值。
    //    不做此步则连接测试与持久化都会把密钥当空处理（401 missing_api_key 的根因）。
    let saved_config = crate::config::Config::from_file(&state.config_path).ok();
    req.actor.api_key = resolve_api_key(
        &req.actor.api_key,
        saved_config.as_ref().and_then(|c| c.llm.api_key.as_deref()),
    );
    if let Some(ref mut reflector) = req.reflector {
        reflector.api_key = resolve_api_key(
            &reflector.api_key,
            saved_config
                .as_ref()
                .and_then(|c| c.llm_reflector.as_ref())
                .and_then(|r| r.api_key.as_deref()),
        );
    }

    // 1. 验证 actor 配置
    if let Err(e) = validate_llm_config(
        &req.actor.provider,
        &req.actor.model,
        req.actor.base_url.as_deref(),
        if req.actor.api_key.is_empty() {
            None
        } else {
            Some(&req.actor.api_key)
        },
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(LlmConfigUpdateResponse {
                success: false,
                message: format!("Actor 配置验证失败: {}", e),
                config: None,
            }),
        )
            .into_response();
    }

    // 2. 验证 reflector 配置（如果有）
    if let Some(ref reflector) = req.reflector
        && let Err(e) = validate_llm_config(
            &reflector.provider,
            &reflector.model,
            reflector.base_url.as_deref(),
            if reflector.api_key.is_empty() {
                None
            } else {
                Some(&reflector.api_key)
            },
        )
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(LlmConfigUpdateResponse {
                success: false,
                message: format!("Reflector 配置验证失败: {}", e),
                config: None,
            }),
        )
            .into_response();
    }

    // 2.5 场景路由校验（键拼错静默失效是已知坑，必须在写入前拦截）
    if let Some(ref routing) = req.scenario_routing
        && let Err(msg) = crate::component::llm::scenario::validate_routing(routing)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(LlmConfigUpdateResponse {
                success: false,
                message: format!("场景路由配置验证失败: {msg}"),
                config: None,
            }),
        )
            .into_response();
    }

    // 2.6 从模型校验：mode=custom 时配置形状必须合法（不做在线连接测试，
    // 运行期从链失败自动回退主链兑底）
    if let Some(ref secondary) = req.llm_secondary {
        if secondary.mode != "mirror" && secondary.mode != "custom" {
            return (
                StatusCode::BAD_REQUEST,
                Json(LlmConfigUpdateResponse {
                    success: false,
                    message: format!(
                        "llm_secondary.mode 非法: {}（仅支持 mirror / custom）",
                        secondary.mode
                    ),
                    config: None,
                }),
            )
                .into_response();
        }
        if secondary.mode == "custom" {
            let Some(ref sec_cfg) = secondary.config else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(LlmConfigUpdateResponse {
                        success: false,
                        message: "从模型 mode=custom 时必须提供 config".to_string(),
                        config: None,
                    }),
                )
                    .into_response();
            };
            let saved_secondary_key = crate::config::Config::from_file(&state.config_path)
                .ok()
                .and_then(|c| c.llm_secondary)
                .and_then(|s| s.api_key.clone());
            let resolved_key = resolve_api_key(&sec_cfg.api_key, saved_secondary_key.as_deref());
            if let Err(e) = validate_llm_config(
                &sec_cfg.provider,
                &sec_cfg.model,
                sec_cfg.base_url.as_deref(),
                if resolved_key.is_empty() {
                    None
                } else {
                    Some(&resolved_key)
                },
            ) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(LlmConfigUpdateResponse {
                        success: false,
                        message: format!("从模型配置验证失败: {e}"),
                        config: None,
                    }),
                )
                    .into_response();
            }
        }
    }

    // 3. 创建测试 LLM 客户端并测试连接
    let provider = match LlmProvider::parse(&req.actor.provider) {
        Some(p) => p,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(LlmConfigUpdateResponse {
                    success: false,
                    message: format!("不支持的 Provider: {}", req.actor.provider),
                    config: None,
                }),
            )
                .into_response();
        }
    };

    let test_config = DirectLlmClientConfig::new(
        provider,
        if req.actor.api_key.is_empty() {
            None::<String>
        } else {
            Some(req.actor.api_key.clone())
        },
    )
    .with_model(&req.actor.model)
    .with_context_window_tokens(req.actor.context_window_tokens.unwrap_or(32768));

    let test_config = if let Some(ref url) = req.actor.base_url {
        test_config.with_base_url(url)
    } else {
        test_config
    };

    let test_client = match DirectLlmClient::new(test_config) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(LlmConfigUpdateResponse {
                    success: false,
                    message: format!("创建 LLM 客户端失败: {}", e),
                    config: None,
                }),
            )
                .into_response();
        }
    };

    // 测试 LLM 连接
    match test_client
        .complete("Hello, this is a connection test. Reply with 'OK'.")
        .await
    {
        Ok(_) => {
            info!(
                "[llm] LLM 连接测试成功: provider={}, model={}",
                req.actor.provider, req.actor.model
            );
        }
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(LlmConfigUpdateResponse {
                    success: false,
                    message: format!("LLM 连接测试失败: {}", e),
                    config: None,
                }),
            )
                .into_response();
        }
    }

    // 4. 读取现有配置
    let mut config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(LlmConfigUpdateResponse {
                    success: false,
                    message: format!("读取配置文件失败: {}", e),
                    config: None,
                }),
            )
                .into_response();
        }
    };

    // 5. 备份原配置
    let backup = config.clone();

    // 6. 更新 LLM 配置
    apply_llm_update(&mut config.llm, &req.actor);

    // 场景路由全量替换（None = 保持现状；Some 含空 map = 清空全部回默认）
    if let Some(routing) = req.scenario_routing {
        config.llm.scenario_routing = routing;
    }

    // 从模型：mirror = 清除独立定义（动态跟随主）；custom = 完整重定义。
    // 先于 reflector 块：reflector 克隆 actor 配置时不受从模型影响，
    // 但保持「先路由后 reflector」的既有顺序。
    match req.llm_secondary.as_ref() {
        Some(update) if update.mode == "mirror" => {
            config.llm_secondary = None;
        }
        Some(update) => {
            let Some(ref sec) = update.config else {
                // 校验层已拦 mode=custom 无 config；到此为防御兑底
                return (
                    StatusCode::BAD_REQUEST,
                    Json(LlmConfigUpdateResponse {
                        success: false,
                        message: "从模型 mode=custom 时必须提供 config".to_string(),
                        config: None,
                    }),
                )
                    .into_response();
            };
            let mut secondary = config.llm.clone();
            // 备用链交由 DTO 的 fallback_models 定义（与主模型同构）；
            // models（per-model 独立配置）无 DTO 通道，克隆副本清空
            secondary.models.clear();
            apply_llm_update(&mut secondary, sec);
            // 密钥留空的回填链：既有从模型密钥 → 同 Provider 时复用主模型密钥
            // （面板承诺「同 Provider 密钥可留空」，骨架克隆的密钥会被
            //  apply_llm_update 的空串置 None 抹掉，必须在此恢复）
            if secondary.api_key.as_deref().unwrap_or("").trim().is_empty() {
                let saved_key = backup
                    .llm_secondary
                    .as_ref()
                    .and_then(|s| s.api_key.clone());
                secondary.api_key = saved_key.or_else(|| {
                    (secondary.provider == backup.llm.provider)
                        .then(|| backup.llm.api_key.clone())
                        .flatten()
                });
            }
            // 从链不消费路由表（路由只认 config.llm），清掉克隆副本避免双事实源
            secondary.scenario_routing.clear();
            config.llm_secondary = Some(secondary);
        }
        // 缺字段 = 保持现状（既有独立从模型原样保留）
        None => {}
    }

    // 更新 reflector 配置
    if req.reflector_inherits_actor {
        config.llm_reflector = None;
    } else if let Some(ref reflector) = req.reflector {
        let mut reflector_config = config.llm.clone();
        reflector_config.fallback_models.clear();
        reflector_config.models.clear();
        // reflector 链不消费路由表，清掉克隆副本避免双事实源
        reflector_config.scenario_routing.clear();
        apply_llm_update(&mut reflector_config, reflector);
        config.llm_reflector = Some(reflector_config);
    }

    // 7. 保存配置（save_to_file 已内置原子写入）
    if let Err(e) = config.save_to_file(&state.config_path) {
        error!("[llm] 保存配置文件失败: {}", e);
        // 尝试恢复备份
        if let Err(e) = backup.save_to_file(&state.config_path) {
            tracing::warn!(
                "llm_config: 备份文件保存失败（旧配置保留，但已修改的新配置生效）：{e:?}"
            );
        }
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(LlmConfigUpdateResponse {
                success: false,
                message: format!("保存配置失败: {}", e),
                config: None,
            }),
        )
            .into_response();
    }

    info!(
        "[llm] LLM 配置已更新: provider={}, model={}",
        req.actor.provider, req.actor.model
    );

    // 8. 返回更新后的配置
    let actor = llm_config_to_info(&config.llm);
    let reflector = config.llm_reflector.as_ref().map(llm_config_to_info);

    let response = dto::LlmConfigResponse {
        actor,
        reflector,
        reflector_inherits_actor: config.llm_reflector.is_none(),
        runtime_mode: state.runtime_mode.to_string(),
        llm_secondary: config.llm_secondary.as_ref().map(llm_config_to_info),
        scenario_routing: effective_scenario_routing(&config.llm),
        scenario_keys: scenario_keys_json(),
    };

    (
        StatusCode::OK,
        Json(LlmConfigUpdateResponse {
            success: true,
            message: "LLM 配置已更新".to_string(),
            config: Some(response),
        }),
    )
        .into_response()
}

/// GET /api/v1/config/llm/usage - 获取 LLM Token 累计使用统计
pub(crate) async fn get_llm_usage_handler() -> impl IntoResponse {
    Json(crate::component::llm::snapshot_all_stats())
}

// ============================================================================

#[cfg(test)]
#[path = "llm_config_tests.rs"]
mod tests;
