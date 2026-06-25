//! LLM 配置管理 API（独立于配置编辑器）
//!
//! 提供友好的表单式 LLM 配置界面，与通用 YAML 编辑器区分

use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::SocketAddr;
use std::time::Duration;

use cyber_jianghu_protocol::{DEFAULT_CONTEXT_WINDOW_TOKENS, DEFAULT_LLM_MAX_TOKENS};

/// LLM 配置数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// 是否启用 LLM 生成
    pub enabled: bool,
    /// Provider: openai / openai_compatible / ollama
    pub provider: String,
    /// API 地址
    pub base_url: String,
    /// API 密钥
    pub api_key: String,
    /// 模型名称
    pub model: String,
    /// 温度参数
    pub temperature: f32,
    /// 最大 token 数
    pub max_tokens: i32,
    /// 上下文窗口大小
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: u32,
}

fn default_context_window_tokens() -> u32 {
    DEFAULT_CONTEXT_WINDOW_TOKENS
}

/// 完整 LLM 配置包装（包含 meta 信息）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfigWrapper {
    pub version: String,
    pub description: String,
    pub meta: LlmMeta,
    pub data: LlmConfig,
}

/// 前端表单读取响应。不要把真实 API key 返回给浏览器。
#[derive(Debug, Clone, Serialize)]
pub struct LlmConfigResponse {
    pub version: String,
    pub description: String,
    pub meta: LlmMeta,
    pub data: LlmConfigPublic,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmConfigPublic {
    pub enabled: bool,
    pub provider: String,
    pub base_url: String,
    pub has_api_key: bool,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: i32,
    pub context_window_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMeta {
    created_at: String,
    author: String,
}

impl Default for LlmConfigWrapper {
    fn default() -> Self {
        Self {
            version: "0.0.1".to_string(),
            description: "LLM 配置，用于群像传记的 LLM 增强生成".to_string(),
            meta: LlmMeta {
                created_at: chrono::Utc::now().format("%Y-%m-%d").to_string(),
                author: "Cyber-Jianghu".to_string(),
            },
            data: LlmConfig {
                enabled: false,
                provider: "ollama".to_string(),
                base_url: "http://localhost:11434/v1".to_string(),
                api_key: String::new(),
                model: "qwen2.5:14b".to_string(),
                temperature: 0.8,
                max_tokens: DEFAULT_LLM_MAX_TOKENS as i32,
                context_window_tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
            },
        }
    }
}

fn parse_llm_config(content: &str) -> Result<LlmConfigWrapper, StatusCode> {
    serde_yaml::from_str(content).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn read_llm_config_raw() -> Result<LlmConfigWrapper, StatusCode> {
    let config_path = crate::paths::get_config_dir().join("llm.yaml");
    if config_path.exists() {
        let content =
            fs::read_to_string(&config_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        parse_llm_config(&content)
    } else {
        Ok(LlmConfigWrapper::default())
    }
}

fn public_llm_config(config: LlmConfigWrapper) -> LlmConfigResponse {
    LlmConfigResponse {
        version: config.version,
        description: config.description,
        meta: config.meta,
        data: LlmConfigPublic {
            enabled: config.data.enabled,
            provider: config.data.provider,
            base_url: config.data.base_url,
            has_api_key: !config.data.api_key.is_empty(),
            model: config.data.model,
            temperature: config.data.temperature,
            max_tokens: config.data.max_tokens,
            context_window_tokens: config.data.context_window_tokens,
        },
    }
}

fn json_err(status: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({ "success": false, "message": msg })),
    )
}

/// GET /api/config/llm - 读取 LLM 配置
pub async fn get_llm_config()
-> Result<Json<LlmConfigResponse>, (StatusCode, Json<serde_json::Value>)> {
    read_llm_config_raw()
        .map(|c| Json(public_llm_config(c)))
        .map_err(|e| json_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))
}

/// POST /api/config/llm - 保存 LLM 配置
pub async fn save_llm_config(
    State(state): State<std::sync::Arc<crate::state::AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(mut config): Json<LlmConfigWrapper>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
    let config_dir = crate::paths::get_config_dir();
    fs::create_dir_all(&config_dir)
        .map_err(|_| json_err(StatusCode::INTERNAL_SERVER_ERROR, "创建配置目录失败"))?;
    let config_path = config_dir.join("llm.yaml");
    let before_state = read_llm_config_raw().ok().map(|existing| {
        serde_json::json!({
            "enabled": existing.data.enabled,
            "provider": existing.data.provider,
            "base_url": existing.data.base_url,
            "model": existing.data.model,
            "temperature": existing.data.temperature,
            "max_tokens": existing.data.max_tokens,
            "context_window_tokens": existing.data.context_window_tokens,
            "has_api_key": !existing.data.api_key.is_empty(),
        })
    });

    if config.data.api_key.is_empty()
        && let Ok(existing) = read_llm_config_raw()
    {
        config.data.api_key = existing.data.api_key;
    }

    let yaml = serde_yaml::to_string(&config)
        .map_err(|_| json_err(StatusCode::INTERNAL_SERVER_ERROR, "序列化配置失败"))?;
    fs::write(&config_path, yaml)
        .map_err(|_| json_err(StatusCode::INTERNAL_SERVER_ERROR, "写入配置文件失败"))?;

    tracing::info!("LLM 配置已保存至 {:?}", config_path);
    if let Err(e) = crate::db::insert_audit_log(
        &state.db_pool,
        crate::db::AuditLogEntry {
            event_type: "llm.config.save",
            actor_type: "admin",
            token_type: Some("write"),
            resource_type: "llm_config",
            resource_id: Some("llm.yaml".to_string()),
            endpoint: "/api/config/llm",
            method: "POST",
            result: "success",
            reason: None,
            payload: serde_json::json!({
                "enabled": config.data.enabled,
                "provider": config.data.provider,
                "base_url": config.data.base_url,
                "model": config.data.model,
                "temperature": config.data.temperature,
                "max_tokens": config.data.max_tokens,
                "context_window_tokens": config.data.context_window_tokens,
                "has_api_key": !config.data.api_key.is_empty(),
            }),
            request_id: Some(audit_ctx.request_id),
            ip: audit_ctx.ip,
            user_agent: audit_ctx.user_agent,
            before_state,
            after_state: Some(serde_json::json!({
                "enabled": config.data.enabled,
                "provider": config.data.provider,
                "base_url": config.data.base_url,
                "model": config.data.model,
                "temperature": config.data.temperature,
                "max_tokens": config.data.max_tokens,
                "context_window_tokens": config.data.context_window_tokens,
                "has_api_key": !config.data.api_key.is_empty(),
            })),
        },
    )
    .await
    {
        tracing::error!("audit_log 写入失败(llm.config.save): {}", e);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "LLM 配置已保存"
    })))
}

/// GET /api/config/llm/status - 检测 LLM 连接状态
pub async fn get_llm_status() -> Json<serde_json::Value> {
    let config_wrapper = match read_llm_config_raw() {
        Ok(c) => c,
        Err(_) => {
            return Json(serde_json::json!({
                "enabled": false,
                "connected": false,
                "message": "配置读取失败"
            }));
        }
    };
    let enabled = config_wrapper.data.enabled;

    if !enabled {
        return Json(serde_json::json!({
            "enabled": false,
            "connected": false,
            "message": "LLM 未启用"
        }));
    }

    // 尝试连接检测
    let base_url = config_wrapper.data.base_url.trim_end_matches('/');
    let check_url = if base_url.contains("/chat/completions") {
        base_url.to_string()
    } else {
        format!("{}/chat/completions", base_url)
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let mut request = client
        .post(&check_url)
        .header("Content-Type", "application/json");

    if !config_wrapper.data.api_key.is_empty() {
        request = request.header(
            "Authorization",
            format!("Bearer {}", config_wrapper.data.api_key),
        );
    }

    let body = serde_json::json!({
        "model": config_wrapper.data.model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 5
    });

    match request.body(body.to_string()).send().await {
        Ok(response) if response.status().is_success() => Json(serde_json::json!({
            "enabled": true,
            "connected": true,
            "message": "连接正常"
        })),
        Ok(response) => Json(serde_json::json!({
            "enabled": true,
            "connected": false,
            "message": format!("连接失败: HTTP {}", response.status().as_u16())
        })),
        Err(_e) => Json(serde_json::json!({
            "enabled": true,
            "connected": false,
            "message": "连接失败".to_string()
        })),
    }
}

/// GET /api/config/llm/enabled - 获取 LLM 启用状态
pub async fn get_llm_enabled() -> Json<serde_json::Value> {
    let config = read_llm_config_raw().unwrap_or_default();
    Json(serde_json::json!({
        "enabled": config.data.enabled
    }))
}

/// POST /api/config/llm/enabled - 设置 LLM 启用状态
pub async fn set_llm_enabled(
    State(state): State<std::sync::Arc<crate::state::AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let audit_ctx = crate::db::build_audit_request_context(&headers, addr);
    let enabled = req
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 读取现有配置
    let config_wrapper = match read_llm_config_raw() {
        Ok(c) => c,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let previous_enabled = config_wrapper.data.enabled;
    let mut config = config_wrapper;
    config.data.enabled = enabled;

    // 保存配置
    let config_dir = crate::paths::get_config_dir();
    fs::create_dir_all(&config_dir).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let config_path = config_dir.join("llm.yaml");
    let yaml = serde_yaml::to_string(&config).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    fs::write(&config_path, yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!("LLM 启用状态已设置为: {}", enabled);
    if let Err(e) = crate::db::insert_audit_log(
        &state.db_pool,
        crate::db::AuditLogEntry {
            event_type: "llm.enabled.set",
            actor_type: "admin",
            token_type: Some("write"),
            resource_type: "llm_config",
            resource_id: Some("llm.yaml".to_string()),
            endpoint: "/api/config/llm/enabled",
            method: "POST",
            result: "success",
            reason: None,
            payload: serde_json::json!({ "enabled": enabled }),
            request_id: Some(audit_ctx.request_id),
            ip: audit_ctx.ip,
            user_agent: audit_ctx.user_agent,
            before_state: Some(serde_json::json!({ "enabled": previous_enabled })),
            after_state: Some(serde_json::json!({ "enabled": enabled })),
        },
    )
    .await
    {
        tracing::error!("audit_log 写入失败(llm.enabled.set): {}", e);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "enabled": enabled
    })))
}

#[cfg(test)]
mod tests {
    use super::parse_llm_config;

    #[test]
    fn test_parse_llm_config_rejects_invalid_yaml() {
        assert!(parse_llm_config("version: [").is_err());
    }
}
