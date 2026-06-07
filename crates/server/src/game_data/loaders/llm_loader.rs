// ============================================================================
// LLM 配置加载器
// ============================================================================

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::RwLock;

use cyber_jianghu_protocol::{DEFAULT_CONTEXT_WINDOW_TOKENS, DEFAULT_LLM_MAX_TOKENS};

use super::config_format::load_config;

/// LLM 配置（与 LlmConfigWrapper.data 保持一致）
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
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
    /// 生成温度
    pub temperature: f64,
    /// 最大 token 数
    pub max_tokens: i32,
    /// HTTP 请求超时（秒）
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// HTTP 连接超时（秒）
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    /// 上下文窗口大小
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: u32,
}

fn default_context_window_tokens() -> u32 {
    DEFAULT_CONTEXT_WINDOW_TOKENS
}

fn default_request_timeout_secs() -> u64 {
    120
}

fn default_connect_timeout_secs() -> u64 {
    30
}

/// 完整 LLM 配置包装（与 config_llm.rs 中的 LlmConfigWrapper 保持一致）
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct LlmConfigWrapper {
    pub version: Option<String>,
    pub description: Option<String>,
    pub meta: Option<LlmConfigMeta>,
    pub data: LlmConfig,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct LlmConfigMeta {
    pub created_at: Option<String>,
    pub author: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "openai_compatible".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "gpt-4o-mini".to_string(),
            temperature: 0.8,
            max_tokens: DEFAULT_LLM_MAX_TOKENS as i32,
            request_timeout_secs: 120,
            connect_timeout_secs: 30,
            context_window_tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
        }
    }
}

/// LLM 配置缓存（进程内单例）
static LLM_CONFIG_CACHE: RwLock<Option<LlmConfig>> = RwLock::new(None);

/// 加载 LLM 配置（带缓存）
///
/// 支持两种格式：
/// 1. LlmConfigWrapper 格式（含 version/description/meta 包装）
/// 2. 直接 LlmConfig 格式（向后兼容）
pub fn load_llm(config_dir: &Path) -> Result<LlmConfig> {
    // 尝试从缓存读取
    if let Some(cached) = LLM_CONFIG_CACHE.read().expect("rwlock poisoned").as_ref() {
        return Ok(cached.clone());
    }

    let config_path = config_dir.join("llm.yaml");
    let json_path = config_dir.join("llm.json");

    let config = if config_path.exists() {
        match load_config::<_, LlmConfigWrapper>(&config_path) {
            Ok(wrapper) => wrapper.data,
            Err(_) => {
                // 尝试直接解析 LlmConfig（向后兼容旧格式）
                load_config::<_, LlmConfig>(&config_path)
                    .context(format!("加载 LLM 配置失败: {}", config_path.display()))?
            }
        }
    } else if json_path.exists() {
        match load_config::<_, LlmConfigWrapper>(&json_path) {
            Ok(wrapper) => wrapper.data,
            Err(_) => load_config::<_, LlmConfig>(&json_path)
                .context(format!("加载 LLM 配置失败: {}", json_path.display()))?,
        }
    } else {
        // 返回默认配置
        LlmConfig::default()
    };

    // 写入缓存
    *LLM_CONFIG_CACHE.write().expect("rwlock poisoned") = Some(config.clone());

    Ok(config)
}

/// 清除 LLM 配置缓存（用于热重载）
#[allow(dead_code)]
pub fn clear_llm_cache() {
    *LLM_CONFIG_CACHE.write().expect("rwlock poisoned") = None;
}
