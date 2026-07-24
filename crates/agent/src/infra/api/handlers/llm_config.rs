// LLM 配置 API Handlers
// ============================================================================

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{error, info};

use cyber_jianghu_protocol::ServerMessage;

use super::HttpApiState;
use super::basic::ErrorResponse;
use super::cognitive_context::{CognitiveContext, CognitiveContextBuilder};
use super::dto;
use axum::http::Response;
use bytes::Bytes;
use http_body::Frame;
use http_body_util::StreamBody;

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
    };

    Json(response).into_response()
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

    // 更新 reflector 配置
    if req.reflector_inherits_actor {
        config.llm_reflector = None;
    } else if let Some(ref reflector) = req.reflector {
        let mut reflector_config = config.llm.clone();
        reflector_config.fallback_models.clear();
        reflector_config.models.clear();
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

// 认知上下文端点
// ============================================================================

/// 认知端点返回的人设信息（从 DynamicPersona 提取）
#[derive(Debug, Serialize)]
pub struct CognitivePersonaInfo {
    pub name: String,
    pub personality: Vec<String>,
    pub description: String,
}

/// 简化的世界状态（用于认知上下文）
#[derive(Debug, Serialize)]
pub struct SimplifiedWorldState {
    pub agent_id: Option<String>,
    pub attributes: std::collections::HashMap<String, i32>,
    pub nearby_entities_count: usize,
    pub time: SimplifiedTime,
}

/// 简化的时间
#[derive(Debug, Serialize)]
pub struct SimplifiedTime {
    pub hour: i32,
    pub weather: String,
}

/// 认知上下文响应
#[derive(Debug, Serialize)]
pub struct CognitiveContextResponse {
    pub cognitive_context: CognitiveContext,
    pub persona: Option<CognitivePersonaInfo>,
    pub world_state: SimplifiedWorldState,
}

/// GET /api/v1/cognitive - 获取结构化认知上下文
///
/// 返回引导 OpenClaw LLM 进行按阶段推理的结构化上下文
pub(crate) async fn get_cognitive_context_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let current = state.current_state.read().await;

    match current.as_ref() {
        Some(world_state) => {
            let builder = CognitiveContextBuilder::new(Default::default());

            let (persona_info, persona_ref): (
                Option<CognitivePersonaInfo>,
                Option<crate::component::persona::dynamic_persona::DynamicPersona>,
            ) = if let Some(ref persona_arc) = state.dynamic_persona {
                persona_arc.read(|p| {
                    let info = CognitivePersonaInfo {
                        name: p.name.clone(),
                        personality: p.traits.keys().take(3).cloned().collect(),
                        description: p.base_description.clone(),
                    };
                    (Some(info), Some(p.clone()))
                })
            } else {
                (None, None)
            };

            let store_arc = state
                .relationship_store
                .read()
                .expect("rwlock poisoned")
                .clone();
            let relationship_store = store_arc.as_deref();
            let cognitive_context =
                builder.build_with_persona(world_state, persona_ref.as_ref(), relationship_store);

            let simplified_world_state = SimplifiedWorldState {
                agent_id: world_state.agent_id.map(|id| id.to_string()),
                attributes: world_state.self_state.attributes.clone(),
                nearby_entities_count: world_state.entities.len(),
                time: SimplifiedTime {
                    hour: world_state.world_time.hour,
                    weather: world_state.world_time.weather.clone(),
                },
            };

            let response = CognitiveContextResponse {
                cognitive_context,
                persona: persona_info,
                world_state: simplified_world_state,
            };

            (StatusCode::OK, Json(response)).into_response()
        }
        None => {
            let error = ErrorResponse {
                error_code: "NO_WORLD_STATE".to_string(),
                message: "No world state available".to_string(),
            };
            (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
        }
    }
}

/// GET /api/v1/events - SSE 实时事件流
///
/// 用于 Web 面板实时接收死亡等事件通知
pub(crate) async fn death_events_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let mut death_rx = state.death_event_tx.subscribe();
    let mut tick_rx = state.tick_update_tx.subscribe();

    let stream = async_stream::stream! {
        let data = Bytes::from_static(b"event: connected\ndata: {\"status\":\"connected\"}\n\n");
        yield Ok::<_, std::convert::Infallible>(Frame::data(data));

        loop {
            tokio::select! {
                death_result = tokio::time::timeout(Duration::from_secs(30), death_rx.recv()) => {
                    match death_result {
                        Ok(Ok(msg)) => {
                            if matches!(msg, ServerMessage::AgentDied { .. })
                                && let Ok(json) = serde_json::to_string(&msg) {
                                let data = Bytes::from(format!("event: agent_died\ndata: {}\n\n", json));
                                yield Ok::<_, std::convert::Infallible>(Frame::data(data));
                            }
                        }
                        Ok(Err(_)) => {
                            break;
                        }
                        Err(_) => {
                            let data = Bytes::from(b"event: heartbeat\ndata: {}\n\n".to_vec());
                            yield Ok::<_, std::convert::Infallible>(Frame::data(data));
                        }
                    }
                }
                tick_result = tick_rx.recv() => {
                    match tick_result {
                        Ok(tick_id) => {
                            let json = serde_json::json!({"tick_id": tick_id}).to_string();
                            let data = Bytes::from(format!("event: tick_update\ndata: {}\n\n", json));
                            yield Ok::<_, std::convert::Infallible>(Frame::data(data));
                        }
                        Err(_) => {
                            break;
                        }
                    }
                }
            }
        }
    };

    let body = StreamBody::new(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .header("X-Accel-Buffering", "no")
        .body(body)
        .expect("valid HTTP response")
}

// ============================================================================

// LLM Metrics
// ============================================================================

/// Query 参数：可选 system_hash hex 过滤
#[derive(Deserialize, Default, Debug, Clone)]
pub struct MetricsQuery {
    pub system_hash: Option<String>,
}

/// GET /api/v1/metrics — LLM 性能指标
pub async fn get_metrics_handler(Query(q): Query<MetricsQuery>) -> Json<serde_json::Value> {
    use crate::component::llm::snapshot_all_stats;

    let mut stats = snapshot_all_stats();
    if let Some(hash_hex) = q.system_hash.as_deref()
        && let Ok(bytes) = hex::decode(hash_hex)
        && bytes.len() == 32
    {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        stats.retain(|s| s.system_hash_distribution.contains_key(&arr));
    }

    let mut total_prompt: u64 = 0;
    let mut total_cache_hit: u64 = 0;

    let models: Vec<serde_json::Value> = stats
        .iter()
        .map(|s| {
            let success_rate = if s.calls > 0 {
                (s.calls - s.failures) as f64 / s.calls as f64
            } else {
                1.0
            };
            let cache_hit_rate = if s.prompt_tokens > 0 {
                s.cache_hit_tokens as f64 / s.prompt_tokens as f64
            } else {
                0.0
            };
            total_prompt += s.prompt_tokens;
            total_cache_hit += s.cache_hit_tokens;
            serde_json::json!({
                "provider": s.provider,
                "model": s.model,
                "calls": s.calls,
                "failures": s.failures,
                "success_rate": format!("{:.0}%", success_rate * 100.0),
                "prompt_tokens": s.prompt_tokens,
                "completion_tokens": s.completion_tokens,
                "total_tokens": s.prompt_tokens + s.completion_tokens,
                "cache_hit_tokens": s.cache_hit_tokens,
                "cache_hit_rate": format!("{:.1}%", cache_hit_rate * 100.0),
            })
        })
        .collect();

    let overall_cache_hit_rate = if total_prompt > 0 {
        total_cache_hit as f64 / total_prompt as f64
    } else {
        0.0
    };

    Json(serde_json::json!({
        "llm": models,
        "total_cache_hit_tokens": total_cache_hit,
        "total_prompt_tokens": total_prompt,
        "cache_hit_rate": format!("{:.1}%", overall_cache_hit_rate * 100.0),
    }))
}

#[cfg(test)]
mod tests {
    use super::resolve_api_key;
    use crate::component::llm::LlmProvider;
    use crate::component::llm::token_tracking::ModelTokenStats;
    use std::collections::HashMap;

    #[test]
    fn metrics_query_filters_by_system_hash() {
        let mut stats = vec![
            ModelTokenStats {
                provider: LlmProvider::OpenAICompatible.as_str().to_string(),
                model: "model-A".to_string(),
                system_hash_distribution: {
                    let mut m = HashMap::new();
                    m.insert([1u8; 32], 5);
                    m.insert([2u8; 32], 3);
                    m
                },
                ..Default::default()
            },
            ModelTokenStats {
                provider: LlmProvider::OpenAICompatible.as_str().to_string(),
                model: "model-B".to_string(),
                system_hash_distribution: {
                    let mut m = HashMap::new();
                    m.insert([3u8; 32], 7);
                    m
                },
                ..Default::default()
            },
        ];

        assert_eq!(stats.len(), 2);

        let target = [1u8; 32];
        stats.retain(|s| s.system_hash_distribution.contains_key(&target));
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].model, "model-A");
    }

    #[test]
    fn metrics_query_hex_decode_32_bytes() {
        let hex_str = "0101010101010101010101010101010101010101010101010101010101010101";
        let bytes = hex::decode(hex_str).unwrap();
        assert_eq!(bytes.len(), 32);
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        assert_eq!(arr, [1u8; 32]);
    }

    #[test]
    fn resolve_api_key_uses_request_when_non_empty() {
        // 用户重新输入密钥 → 用新值
        assert_eq!(resolve_api_key("sk-new", Some("sk-old")), "sk-new");
    }

    #[test]
    fn resolve_api_key_trims_request_value() {
        // 前端 trim 后发送，后端再次 trim 保持幂等
        assert_eq!(resolve_api_key("  sk-new  ", Some("sk-old")), "sk-new");
    }

    #[test]
    fn resolve_api_key_falls_back_to_saved_when_request_empty() {
        // 用户未修改密钥（空串）→ 复用已保存值（401 missing_api_key 根因修复）
        assert_eq!(resolve_api_key("", Some("sk-saved")), "sk-saved");
    }

    #[test]
    fn resolve_api_key_falls_back_to_saved_when_request_blank() {
        // 纯空白也视为"未修改"
        assert_eq!(resolve_api_key("   ", Some("sk-saved")), "sk-saved");
    }

    #[test]
    fn resolve_api_key_trims_saved_value() {
        assert_eq!(resolve_api_key("", Some("  sk-saved  ")), "sk-saved");
    }

    #[test]
    fn resolve_api_key_returns_empty_when_both_empty() {
        // 两者皆空 → 空串（下游既有"空串→None"语义保持不变）
        assert_eq!(resolve_api_key("", None), "");
    }

    #[test]
    fn resolve_api_key_returns_empty_when_saved_is_blank() {
        // 已保存值为空串/纯空白 → 视为无密钥
        assert_eq!(resolve_api_key("", Some("")), "");
        assert_eq!(resolve_api_key("", Some("   ")), "");
    }

    #[test]
    fn resolve_api_key_request_takes_precedence_over_blank_saved() {
        // 即使 saved 为空，req 非空仍用 req
        assert_eq!(resolve_api_key("sk-new", Some("")), "sk-new");
    }
}

// ============================================================================
