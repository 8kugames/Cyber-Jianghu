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

/// LLM 配置的环境变量注入入口（优先于 llm.yaml 对应字段）。
///
/// llm.yaml 是可入库模板：连接五元组全部留空，运行时经环境变量提供，
/// 密钥永不落配置文件。
pub const LLM_ENABLED_ENV: &str = "CYBER_JIANGHU_LLM_ENABLED";
pub const LLM_PROVIDER_ENV: &str = "CYBER_JIANGHU_LLM_PROVIDER";
pub const LLM_BASE_URL_ENV: &str = "CYBER_JIANGHU_LLM_BASE_URL";
pub const LLM_API_KEY_ENV: &str = "CYBER_JIANGHU_LLM_API_KEY";
pub const LLM_MODEL_ENV: &str = "CYBER_JIANGHU_LLM_MODEL";

/// 环境变量注入值（生产路径从进程环境收集，测试直接构造）
#[derive(Debug, Clone, Default)]
pub struct LlmEnvOverrides {
    pub enabled: Option<bool>,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
}

impl LlmEnvOverrides {
    fn from_env() -> Self {
        let read = |key: &str| {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let enabled = read(LLM_ENABLED_ENV).and_then(|v| match v.to_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => {
                tracing::warn!("{} 值无法解析为布尔: {}（忽略）", LLM_ENABLED_ENV, v);
                None
            }
        });
        Self {
            enabled,
            provider: read(LLM_PROVIDER_ENV),
            base_url: read(LLM_BASE_URL_ENV),
            api_key: read(LLM_API_KEY_ENV),
            model: read(LLM_MODEL_ENV),
        }
    }

    /// 应用覆盖：env 非空字段优先于文件值
    fn apply_to(self, config: &mut LlmConfig) {
        if let Some(v) = self.enabled {
            config.enabled = v;
        }
        if let Some(v) = self.provider {
            config.provider = v;
        }
        if let Some(v) = self.base_url {
            config.base_url = v;
        }
        if let Some(v) = self.api_key {
            config.api_key = v;
        }
        if let Some(v) = self.model {
            config.model = v;
        }
    }
}

/// 加载 LLM 配置（带缓存）
///
/// 支持两种格式：
/// 1. LlmConfigWrapper 格式（含 version/description/meta 包装）
/// 2. 直接 LlmConfig 格式（向后兼容）
pub fn load_llm(config_dir: &Path) -> Result<LlmConfig> {
    load_llm_with_env(config_dir, LlmEnvOverrides::from_env())
}

/// 内层加载：overrides 由调用方注入（生产路径读环境变量，测试直接构造）
fn load_llm_with_env(config_dir: &Path, env: LlmEnvOverrides) -> Result<LlmConfig> {
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

    // 环境变量注入：非空字段优先于文件值（密钥永不落配置文件）
    env.apply_to(&mut config);

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
            "version: 0.0.1\ndata:\n  enabled: true\n  provider: file-provider\n  base_url: https://file.example/v1\n  api_key: file-key\n  model: file-model\n  temperature: 0.8\n  max_tokens: 100\n",
        )
        .unwrap();
    }

    #[test]
    fn env_overrides_file_connection_fields() {
        let dir = std::env::temp_dir().join(format!("cj-llm-loader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_wrapper_config(&dir);

        clear_llm_cache();
        let cfg = load_llm_with_env(
            &dir,
            LlmEnvOverrides {
                api_key: Some("env-key".to_string()),
                model: Some("env-model".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cfg.api_key, "env-key");
        assert_eq!(cfg.model, "env-model");
        // 未注入字段保留文件值
        assert_eq!(cfg.provider, "file-provider");
        assert_eq!(cfg.base_url, "https://file.example/v1");
        assert!(cfg.enabled);

        clear_llm_cache();
        let cfg = load_llm_with_env(&dir, LlmEnvOverrides::default()).unwrap();
        assert_eq!(cfg.api_key, "file-key");

        clear_llm_cache();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_overrides_apply_when_config_file_missing() {
        let dir =
            std::env::temp_dir().join(format!("cj-llm-loader-missing-{}", std::process::id()));
        clear_llm_cache();
        let cfg = load_llm_with_env(
            &dir,
            LlmEnvOverrides {
                enabled: Some(true),
                api_key: Some("env-key".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.api_key, "env-key");
        clear_llm_cache();
    }
}
