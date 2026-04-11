//! LLM 配置管理 API（独立于配置编辑器）
//!
//! 提供友好的表单式 LLM 配置界面，与通用 YAML 编辑器区分

use axum::Json;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::fs;

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
}

/// 完整 LLM 配置包装（包含 meta 信息）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfigWrapper {
    pub version: String,
    pub description: String,
    pub meta: LlmMeta,
    pub data: LlmConfig,
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
                max_tokens: 4096,
            },
        }
    }
}

/// GET /api/config/llm - 读取 LLM 配置
pub async fn get_llm_config() -> Result<Json<LlmConfigWrapper>, StatusCode> {
    let config_path = crate::paths::get_config_dir().join("llm.yaml");
    let wrapper = if config_path.exists() {
        let content = fs::read_to_string(&config_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        serde_yaml::from_str(&content).unwrap_or_default()
    } else {
        LlmConfigWrapper::default()
    };

    Ok(Json(wrapper))
}

/// POST /api/config/llm - 保存 LLM 配置
pub async fn save_llm_config(
    Json(config): Json<LlmConfigWrapper>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let config_dir = crate::paths::get_config_dir();
    fs::create_dir_all(&config_dir).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let config_path = config_dir.join("llm.yaml");

    let yaml = serde_yaml::to_string(&config).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    fs::write(&config_path, yaml).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    tracing::info!("LLM 配置已保存至 {:?}", config_path);

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "LLM 配置已保存"
    })))
}
