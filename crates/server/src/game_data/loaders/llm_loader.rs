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

/// 环境变量名：LLM API 密钥的注入入口（优先于 llm.yaml 中的 api_key）
///
/// 密钥不落配置文件：llm.yaml 的 api_key 留空，真实密钥经 .env / 部署环境注入。
pub const LLM_API_KEY_ENV: &str = "CYBER_JIANGHU_LLM_API_KEY";

/// 加载 LLM 配置（带缓存）
///
/// 支持两种格式：
/// 1. LlmConfigWrapper 格式（含 version/description/meta 包装）
/// 2. 直接 LlmConfig 格式（向后兼容）
pub fn load_llm(config_dir: &Path) -> Result<LlmConfig> {
    let env_api_key = std::env::var(LLM_API_KEY_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    load_llm_with_env(config_dir, env_api_key.as_deref())
}

/// 内层加载：env_api_key 由调用方注入（生产路径读环境变量，测试直接传参）
fn load_llm_with_env(config_dir: &Path, env_api_key: Option<&str>) -> Result<LlmConfig> {
    // 尝试从缓存读取
    if let Some(cached) = LLM_CONFIG_CACHE.read().expect("rwlock poisoned").as_ref() {
        return Ok(cached.clone());
    }

    let config_path = config_dir.join("llm.yaml");
    let json_path = config_dir.join("llm.json");

    let mut config = if config_path.exists() {
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

    // 密钥注入：环境变量优先于文件值（llm.yaml 的 api_key 应留空）
    if let Some(key) = env_api_key {
        tracing::info!(
            "LLM api_key 来自环境变量 {}（不落配置文件）",
            LLM_API_KEY_ENV
        );
        config.api_key = key.to_string();
    }

    // 写入缓存
    *LLM_CONFIG_CACHE.write().expect("rwlock poisoned") = Some(config.clone());

    Ok(config)
}

/// 清除 LLM 配置缓存（用于热重载）
#[allow(dead_code)]
pub fn clear_llm_cache() {
    *LLM_CONFIG_CACHE.write().expect("rwlock poisoned") = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wrapper_config(dir: &Path) {
        std::fs::write(
            dir.join("llm.yaml"),
            "version: 0.0.1\ndata:\n  enabled: true\n  provider: openai_compatible\n  base_url: https://example.com/v1\n  api_key: file-key\n  model: test-model\n  temperature: 0.8\n  max_tokens: 100\n",
        )
        .unwrap();
    }

    #[test]
    fn env_api_key_overrides_file_value() {
        let dir = std::env::temp_dir().join(format!("cj-llm-loader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_wrapper_config(&dir);

        clear_llm_cache();
        let cfg = load_llm_with_env(&dir, Some("env-key")).unwrap();
        assert_eq!(cfg.api_key, "env-key");

        clear_llm_cache();
        let cfg = load_llm_with_env(&dir, None).unwrap();
        assert_eq!(cfg.api_key, "file-key");

        clear_llm_cache();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_api_key_applies_when_config_file_missing() {
        let dir =
            std::env::temp_dir().join(format!("cj-llm-loader-missing-{}", std::process::id()));
        clear_llm_cache();
        let cfg = load_llm_with_env(&dir, Some("env-key")).unwrap();
        assert_eq!(cfg.api_key, "env-key");
        clear_llm_cache();
    }
}
