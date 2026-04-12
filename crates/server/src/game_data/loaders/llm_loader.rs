// ============================================================================
// LLM 配置加载器
// ============================================================================

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::RwLock;

use super::config_format::load_config;

/// LLM 配置
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
            max_tokens: 2000,
        }
    }
}

/// LLM 配置缓存（进程内单例）
static LLM_CONFIG_CACHE: RwLock<Option<LlmConfig>> = RwLock::new(None);

/// 加载 LLM 配置（带缓存）
pub fn load_llm(config_dir: &Path) -> Result<LlmConfig> {
    // 尝试从缓存读取
    if let Some(cached) = LLM_CONFIG_CACHE.read().unwrap().as_ref() {
        return Ok(cached.clone());
    }

    let config_path = config_dir.join("llm.yaml");
    let json_path = config_dir.join("llm.json");

    let config = if config_path.exists() {
        load_config(&config_path)
            .context(format!("加载 LLM 配置失败: {}", config_path.display()))?
    } else if json_path.exists() {
        load_config(&json_path).context(format!("加载 LLM 配置失败: {}", json_path.display()))?
    } else {
        // 返回默认配置
        LlmConfig::default()
    };

    // 写入缓存
    *LLM_CONFIG_CACHE.write().unwrap() = Some(config.clone());

    Ok(config)
}

/// 清除 LLM 配置缓存（用于热重载）
#[allow(dead_code)]
pub fn clear_llm_cache() {
    *LLM_CONFIG_CACHE.write().unwrap() = None;
}
