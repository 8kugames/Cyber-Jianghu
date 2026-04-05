// ============================================================================
// Direct LLM Client
// ============================================================================
//
// 直接调用 LLM Provider API
//
// 支持的 Provider:
// - openclaw: 使用宿主 OpenClaw 已配置（读取 ~/.openclaw/openclaw.json）
// - openai_compatible: 兼容 OpenAI 接口（需要手动指定 URL 和模型）
// - ollama: Ollama 本地部署
// ============================================================================

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tracing::{debug, error};

use super::LlmClient;

// ============================================================================
// Token Usage Tracking (per provider-model)
// ============================================================================

/// Per-model token stats
struct PerModelStats {
    prompt_tokens: u64,
    completion_tokens: u64,
    calls: u64,
}

impl PerModelStats {
    fn new() -> Self {
        Self {
            prompt_tokens: 0,
            completion_tokens: 0,
            calls: 0,
        }
    }

    fn record(&mut self, prompt: u64, completion: u64) {
        self.prompt_tokens += prompt;
        self.completion_tokens += completion;
        self.calls += 1;
    }
}

/// Token stats for a specific provider-model key
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTokenStats {
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub calls: u64,
}

static TOKEN_STATS: OnceLock<Mutex<HashMap<String, PerModelStats>>> = OnceLock::new();

fn token_stats() -> &'static Mutex<HashMap<String, PerModelStats>> {
    TOKEN_STATS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn model_key(provider: &LlmProvider, model: &str) -> String {
    format!("{}/{}", provider.as_str(), model)
}

const TOKEN_LOG_DIR: &str = ".cyber-jianghu/logs";
const TOKEN_LOG_FILE: &str = "token_cost_count.tmp";

fn log_file_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(TOKEN_LOG_DIR).join(TOKEN_LOG_FILE))
}

/// Record token usage for a specific provider-model
pub fn record_token_usage(
    provider: &LlmProvider,
    model: &str,
    prompt_tokens: u64,
    completion_tokens: u64,
) {
    let key = model_key(provider, model);
    if let Ok(mut stats) = token_stats().lock() {
        stats
            .entry(key)
            .or_insert_with(PerModelStats::new)
            .record(prompt_tokens, completion_tokens);
    }
}

/// Get snapshot of all model stats (does not clear)
pub fn snapshot_all_stats() -> Vec<ModelTokenStats> {
    let Ok(stats) = token_stats().lock() else {
        return vec![];
    };
    stats
        .iter()
        .map(|(key, s)| {
            let parts: Vec<&str> = key.splitn(2, '/').collect();
            let (provider, model) = if parts.len() == 2 {
                (parts[0].to_string(), parts[1].to_string())
            } else {
                ("unknown".to_string(), key.clone())
            };
            ModelTokenStats {
                provider,
                model,
                prompt_tokens: s.prompt_tokens,
                completion_tokens: s.completion_tokens,
                total_tokens: s.prompt_tokens + s.completion_tokens,
                calls: s.calls,
            }
        })
        .collect()
}

/// Persist all stats to file and reset counters
pub fn persist_and_reset() {
    let stats = snapshot_all_stats();
    if stats.is_empty() {
        return;
    }
    if let Some(path) = log_file_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // Read existing data
        let existing: HashMap<String, ModelTokenStats> = if path.exists() {
            let content = fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };
        // Merge: add to existing counts
        let mut merged: HashMap<String, ModelTokenStats> = existing;
        for s in &stats {
            let key = format!("{}/{}", s.provider, s.model);
            if let Some(existing) = merged.get_mut(&key) {
                existing.prompt_tokens += s.prompt_tokens;
                existing.completion_tokens += s.completion_tokens;
                existing.total_tokens += s.total_tokens;
                existing.calls += s.calls;
            } else {
                merged.insert(key, s.clone());
            }
        }
        // Write back
        if let Ok(json) = serde_json::to_string_pretty(&merged) {
            let _ = fs::write(&path, json);
        }
    }
    // Reset current tick counters
    if let Ok(mut stats) = token_stats().lock() {
        stats.clear();
    }
}

/// OpenClaw 配置文件格式
#[derive(Debug, Deserialize)]
pub struct OpenClawConfig {
    /// Gateway 配置
    #[serde(default)]
    gateway: Option<GatewayConfig>,
}

#[derive(Debug, Deserialize)]
struct GatewayConfig {
    /// Gateway 地址
    url: Option<String>,
}

impl OpenClawConfig {
    /// 从默认路径读取配置
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        let content = std::fs::read_to_string(&config_path).with_context(|| {
            format!(
                "Failed to read OpenClaw config from {}",
                config_path.display()
            )
        })?;
        serde_json::from_str(&content).with_context(|| {
            format!(
                "Failed to parse OpenClaw config from {}",
                config_path.display()
            )
        })
    }

    /// 获取配置文件路径
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = std::env::var("HOME")
            .map(|home| PathBuf::from(home).join(".openclaw"))
            .unwrap_or_else(|_| PathBuf::from("."));

        Ok(config_dir.join("openclaw.json"))
    }

    /// 获取 Gateway URL
    pub fn gateway_url(&self) -> Option<&String> {
        self.gateway.as_ref()?.url.as_ref()
    }
}

/// LLM Provider 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    /// 使用宿主 OpenClaw 已配置（通过 OpenClaw Gateway）
    OpenClaw,
    /// 兼容 OpenAI 接口（需要手动指定 URL 和模型）
    OpenAICompatible,
    /// Ollama 本地部署
    Ollama,
}

impl LlmProvider {
    /// 获取 provider 的字符串表示
    pub fn as_str(&self) -> &str {
        match self {
            LlmProvider::OpenClaw => "openclaw",
            LlmProvider::OpenAICompatible => "openai_compatible",
            LlmProvider::Ollama => "ollama",
        }
    }

    /// 从字符串解析
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "openclaw" => Some(Self::OpenClaw),
            "openai_compatible" | "openai-compatible" => Some(Self::OpenAICompatible),
            "ollama" => Some(Self::Ollama),
            _ => None,
        }
    }

    /// 默认 Base URL（如果有的话）
    fn default_base_url(&self) -> Option<&'static str> {
        match self {
            Self::OpenClaw => None,         // 从配置文件读取
            Self::OpenAICompatible => None, // 必须手动指定
            Self::Ollama => Some("http://localhost:11434/v1"),
        }
    }

    /// 默认模型（如果有的话）
    fn default_model(&self) -> Option<&'static str> {
        match self {
            Self::OpenClaw => None,         // 从配置文件读取
            Self::OpenAICompatible => None, // 必须手动指定
            Self::Ollama => None,           // 不指定默认模型
        }
    }

    /// 是否需要 API Key
    pub fn requires_api_key(&self) -> bool {
        match self {
            Self::OpenClaw => true, // OpenClaw 读取 Gateway 配置，但 API Key 需用户输入
            Self::OpenAICompatible => true, // OpenAI 兼容接口通常需要 key
            Self::Ollama => false,  // Ollama 本地通常不需要
        }
    }

    /// 是否需要手动指定 Base URL
    pub fn requires_base_url(&self) -> bool {
        matches!(self, Self::OpenAICompatible)
    }

    /// 是否需要手动指定模型
    pub fn requires_model(&self) -> bool {
        matches!(self, Self::OpenAICompatible)
    }

    /// 是否从配置文件读取
    pub fn reads_from_config(&self) -> bool {
        matches!(self, Self::OpenClaw)
    }
}

/// Direct LLM 客户端配置
#[derive(Clone, Debug)]
pub struct DirectLlmClientConfig {
    /// Provider 类型
    pub provider: LlmProvider,
    /// API Base URL（某些 provider 必须手动指定）
    pub base_url: Option<String>,
    /// API Key（部分 provider 不需要）
    pub api_key: Option<String>,
    /// 模型名称（某些 provider 必须手动指定）
    pub model: Option<String>,
    /// 温度参数 (0.0 - 1.0)
    pub temperature: f32,
    /// 最大 tokens
    pub max_tokens: u32,
}

impl DirectLlmClientConfig {
    /// 创建新的配置
    ///
    /// # 参数
    ///
    /// - `provider`: LLM Provider 类型
    /// - `api_key`: API Key（对于不需要的 provider 可以传 None）
    ///
    /// 注意：
    /// - `OpenAICompatible` 必须通过 `with_base_url` 和 `with_model` 指定 URL 和模型
    /// - `OpenClaw` 会自动读取 ~/.openclaw/openclaw.json 配置
    /// - `Ollama` 可以使用默认配置
    pub fn new(provider: LlmProvider, api_key: Option<impl Into<String>>) -> Self {
        Self {
            provider,
            base_url: None,
            api_key: api_key.map(|k| k.into()),
            model: None,
            temperature: 0.7,
            max_tokens: 4096,
        }
    }

    /// 从 OpenClaw 配置文件加载配置（仅对 OpenClaw provider 有效）
    pub fn load_from_openclaw_config(mut self) -> Result<Self> {
        if self.provider != LlmProvider::OpenClaw {
            return Ok(self);
        }

        let config = OpenClawConfig::load().context(
            "Failed to load OpenClaw configuration. Ensure ~/.openclaw/openclaw.json exists.",
        )?;

        if let Some(gateway_url) = config.gateway_url() {
            debug!("Loaded OpenClaw Gateway URL from config: {}", gateway_url);
            self.base_url = Some(gateway_url.clone());
        }

        // OpenClaw 配置文件中包含认证信息，不需要额外的 API key
        // 如果用户提供了 API key，仍然使用（覆盖配置）
        Ok(self)
    }

    /// 设置 Base URL
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// 设置模型名称
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// 设置温度参数
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature.clamp(0.0, 1.0);
        self
    }

    /// 设置最大 tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// 验证配置是否完整
    ///
    /// 返回错误如果缺少必要的配置
    pub fn validate(&self) -> Result<()> {
        if self.provider.requires_base_url() && self.base_url.is_none() {
            anyhow::bail!(
                "Provider {:?} requires --base-url to be specified",
                self.provider
            );
        }
        if self.provider.requires_model() && self.model.is_none() {
            anyhow::bail!(
                "Provider {:?} requires --model to be specified",
                self.provider
            );
        }
        // OpenClaw 不需要验证 base_url 和 model，因为从配置文件读取
        Ok(())
    }

    /// 获取实际的 Base URL
    ///
    /// 返回错误如果 provider 需要但未指定
    pub fn get_base_url(&self) -> Result<String> {
        if let Some(url) = &self.base_url {
            Ok(url.clone())
        } else if let Some(default) = self.provider.default_base_url() {
            Ok(default.to_string())
        } else {
            anyhow::bail!(
                "Provider {:?} requires --base-url to be specified",
                self.provider
            )
        }
    }

    /// 获取实际的模型名称
    ///
    /// 返回错误如果 provider 需要但未指定
    pub fn get_model(&self) -> Result<String> {
        if let Some(model) = &self.model {
            Ok(model.clone())
        } else if let Some(default) = self.provider.default_model() {
            Ok(default.to_string())
        } else {
            anyhow::bail!(
                "Provider {:?} requires --model to be specified",
                self.provider
            )
        }
    }

    /// 获取模型名称（带默认值）
    ///
    /// 对于 OpenClaw，如果未指定模型，返回 "default"（由 Gateway 决定）
    pub fn get_model_with_default(&self) -> String {
        if let Some(model) = &self.model {
            model.clone()
        } else if self.provider.default_model().is_some() {
            self.provider.default_model().unwrap().to_string()
        } else {
            "default".to_string()
        }
    }
}

/// Direct LLM 客户端
///
/// 直接调用 LLM Provider API
#[derive(Clone, Debug)]
pub struct DirectLlmClient {
    config: DirectLlmClientConfig,
}

impl DirectLlmClient {
    /// 创建新的 Direct LLM 客户端
    pub fn new(mut config: DirectLlmClientConfig) -> Result<Self> {
        // 对于 OpenClaw，自动加载配置文件
        if config.provider == LlmProvider::OpenClaw {
            config = config.load_from_openclaw_config()?;
        }
        // 验证配置
        config.validate()?;
        Ok(Self { config })
    }

    /// 获取当前使用的模型名称
    pub fn model_name(&self) -> String {
        self.config.get_model_with_default()
    }

    /// 获取当前使用的 provider 名称
    pub fn provider_name(&self) -> String {
        self.config.provider.as_str().to_string()
    }

    /// 便捷方法：创建 OpenClaw 客户端（自动读取配置文件）
    pub fn openclaw() -> Result<Self> {
        Self::new(DirectLlmClientConfig::new(
            LlmProvider::OpenClaw,
            None::<String>,
        ))
    }

    /// 便捷方法：创建 OpenClaw 客户端（手动指定 Gateway URL）
    pub fn openclaw_with_url(gateway_url: impl Into<String>) -> Result<Self> {
        Self::new(
            DirectLlmClientConfig::new(LlmProvider::OpenClaw, None::<String>)
                .with_base_url(gateway_url),
        )
    }

    /// 便捷方法：创建 OpenAI Compatible 客户端
    ///
    /// 必须指定 base_url 和 model
    pub fn openai_compatible(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self> {
        Self::new(
            DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some(api_key))
                .with_base_url(base_url)
                .with_model(model),
        )
    }

    /// 便捷方法：创建 Ollama 客户端
    pub fn ollama(base_url: Option<impl Into<String>>) -> Result<Self> {
        let mut config = DirectLlmClientConfig::new(LlmProvider::Ollama, None::<String>);
        if let Some(url) = base_url {
            config = config.with_base_url(url);
        }
        Self::new(config)
    }

    /// 构建 HTTP 客户端
    fn build_http_client(&self) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("Failed to build HTTP client")
    }

    /// 调用 OpenAI 兼容 API
    ///
    /// OpenClaw Gateway、OpenAI Compatible、Ollama 都使用 OpenAI 兼容接口
    async fn call_openai_compatible_api(&self, prompt: &str) -> Result<String> {
        let client = self.build_http_client()?;
        let base_url = self.config.get_base_url()?;
        let model = self.config.get_model_with_default();
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

        let request = OpenAIRequest {
            model: model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
        };

        debug!("Calling OpenAI-compatible API: {}", url);
        debug!("Request model: {}", request.model);

        let mut request_builder = client.post(&url).header("Content-Type", "application/json");

        // 添加 Authorization 头（如果有 API Key）
        if let Some(ref api_key) = self.config.api_key {
            request_builder =
                request_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request_builder
            .json(&request)
            .send()
            .await
            .context("Failed to send request to LLM API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            error!("LLM API error {}: {}", status, error_body);
            anyhow::bail!("LLM API error {}: {}", status, error_body);
        }

        let response_data: OpenAIResponse = response
            .json()
            .await
            .context("Failed to parse LLM response")?;

        // 记录 token 使用量
        if let Some(ref usage) = response_data.usage {
            record_token_usage(
                &self.config.provider,
                &model,
                usage.prompt_tokens,
                usage.completion_tokens,
            );
            debug!(
                "Token usage: provider={}, model={}, prompt={}, completion={}",
                self.config.provider.as_str(),
                model,
                usage.prompt_tokens,
                usage.completion_tokens
            );
        }

        if let Some(choice) = response_data.choices.first() {
            let content = choice.message.content.trim().to_string();
            debug!("LLM response length: {} chars", content.len());
            Ok(content)
        } else {
            anyhow::bail!("LLM returned empty response")
        }
    }

    /// 调用 OpenAI 兼容 API（system + user 分离）
    ///
    /// 使用 system role 发送系统指令，user role 发送用户 prompt，
    /// 利用 LLM 的 system message 优先级机制确保角色指令不被截断。
    async fn call_openai_compatible_api_with_system(
        &self,
        system: &str,
        prompt: &str,
    ) -> Result<String> {
        let client = self.build_http_client()?;
        let base_url = self.config.get_base_url()?;
        let model = self.config.get_model_with_default();
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

        let request = OpenAIRequest {
            model: model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: prompt.to_string(),
                },
            ],
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
        };

        debug!("Calling OpenAI-compatible API (system+user): {}", url);
        debug!("Request model: {}", request.model);

        let mut request_builder = client.post(&url).header("Content-Type", "application/json");

        if let Some(ref api_key) = self.config.api_key {
            request_builder =
                request_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request_builder
            .json(&request)
            .send()
            .await
            .context("Failed to send request to LLM API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            error!("LLM API error {}: {}", status, error_body);
            anyhow::bail!("LLM API error {}: {}", status, error_body);
        }

        let response_data: OpenAIResponse = response
            .json()
            .await
            .context("Failed to parse LLM response")?;

        if let Some(ref usage) = response_data.usage {
            record_token_usage(
                &self.config.provider,
                &model,
                usage.prompt_tokens,
                usage.completion_tokens,
            );
            debug!(
                "Token usage: provider={}, model={}, prompt={}, completion={}",
                self.config.provider.as_str(),
                model,
                usage.prompt_tokens,
                usage.completion_tokens
            );
        }

        if let Some(choice) = response_data.choices.first() {
            let content = choice.message.content.trim().to_string();
            debug!("LLM response length: {} chars", content.len());
            Ok(content)
        } else {
            anyhow::bail!("LLM returned empty response")
        }
    }
}

#[async_trait]
impl LlmClient for DirectLlmClient {
    async fn complete(&self, prompt: &str) -> Result<String> {
        // 所有三种 provider 都使用 OpenAI 兼容接口
        self.call_openai_compatible_api(prompt).await
    }

    async fn complete_with_system(&self, system: &str, prompt: &str) -> Result<String> {
        self.call_openai_compatible_api_with_system(system, prompt)
            .await
    }
}

// ============================================================================
// OpenAI 兼容 API 类型
// ============================================================================

#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
    #[serde(default)]
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: ChatMessage,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_from_str() {
        assert_eq!(LlmProvider::parse("openclaw"), Some(LlmProvider::OpenClaw));
        assert_eq!(LlmProvider::parse("OpenClaw"), Some(LlmProvider::OpenClaw));
        assert_eq!(
            LlmProvider::parse("openai_compatible"),
            Some(LlmProvider::OpenAICompatible)
        );
        assert_eq!(
            LlmProvider::parse("openai-compatible"),
            Some(LlmProvider::OpenAICompatible)
        );
        assert_eq!(LlmProvider::parse("ollama"), Some(LlmProvider::Ollama));
        assert_eq!(LlmProvider::parse("unknown"), None);
    }

    #[test]
    fn test_provider_defaults() {
        // OpenClaw 从配置文件读取 base_url/model，但需要用户输入 API Key
        assert_eq!(LlmProvider::OpenClaw.default_base_url(), None);
        assert_eq!(LlmProvider::OpenClaw.default_model(), None);
        assert!(LlmProvider::OpenClaw.requires_api_key()); // 用户需要手动输入
        assert!(!LlmProvider::OpenClaw.requires_base_url()); // 从配置文件读取
        assert!(!LlmProvider::OpenClaw.requires_model()); // 从配置文件读取
        assert!(LlmProvider::OpenClaw.reads_from_config());

        // OpenAICompatible 没有默认值
        assert_eq!(LlmProvider::OpenAICompatible.default_base_url(), None);
        assert_eq!(LlmProvider::OpenAICompatible.default_model(), None);
        assert!(LlmProvider::OpenAICompatible.requires_api_key());
        assert!(LlmProvider::OpenAICompatible.requires_base_url());
        assert!(LlmProvider::OpenAICompatible.requires_model());

        // Ollama 有默认 URL 但没有默认模型
        assert_eq!(
            LlmProvider::Ollama.default_base_url(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(LlmProvider::Ollama.default_model(), None);
        assert!(!LlmProvider::Ollama.requires_api_key());
        assert!(!LlmProvider::Ollama.requires_base_url());
        assert!(!LlmProvider::Ollama.requires_model());
    }

    #[test]
    fn test_config_builder() {
        let config = DirectLlmClientConfig::new(LlmProvider::OpenClaw, Some("test-key"))
            .with_model("custom-model")
            .with_temperature(0.5)
            .with_max_tokens(2048);

        assert_eq!(config.provider, LlmProvider::OpenClaw);
        assert_eq!(config.api_key, Some("test-key".to_string()));
        assert_eq!(config.model, Some("custom-model".to_string()));
        assert_eq!(config.temperature, 0.5);
        assert_eq!(config.max_tokens, 2048);
    }

    #[test]
    fn test_config_validate() {
        // OpenAICompatible 需要 base_url 和 model
        let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"));
        assert!(config.validate().is_err());

        let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"))
            .with_base_url("https://api.example.com");
        assert!(config.validate().is_err()); // 仍然缺少 model

        let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"))
            .with_base_url("https://api.example.com")
            .with_model("gpt-4");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_get_base_url() {
        // Ollama 默认 URL
        let config = DirectLlmClientConfig::new(LlmProvider::Ollama, None::<String>);
        assert_eq!(config.get_base_url().unwrap(), "http://localhost:11434/v1");

        // 覆盖默认 URL
        let config = config.with_base_url("https://custom.api/v1");
        assert_eq!(config.get_base_url().unwrap(), "https://custom.api/v1");

        // OpenAICompatible 没有默认 URL
        let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"))
            .with_model("gpt-4");
        assert!(config.get_base_url().is_err());

        let config = config.with_base_url("https://api.example.com");
        assert_eq!(config.get_base_url().unwrap(), "https://api.example.com");
    }

    #[test]
    fn test_config_get_model() {
        // OpenClaw 返回默认值
        let config = DirectLlmClientConfig::new(LlmProvider::OpenClaw, None::<String>);
        assert_eq!(config.get_model_with_default(), "default");

        // 覆盖模型
        let config = config.with_model("custom-model");
        assert_eq!(config.get_model_with_default(), "custom-model");

        // Ollama 没有默认模型
        let config = DirectLlmClientConfig::new(LlmProvider::Ollama, None::<String>);
        assert_eq!(config.get_model_with_default(), "default");

        // OpenAICompatible 没有默认模型
        let config = DirectLlmClientConfig::new(LlmProvider::OpenAICompatible, Some("test-key"))
            .with_base_url("https://api.example.com");
        assert_eq!(config.get_model_with_default(), "default");

        let config = config.with_model("gpt-4");
        assert_eq!(config.get_model_with_default(), "gpt-4");
    }

    #[test]
    fn test_direct_client_openclaw() {
        // OpenClaw 不需要 API key，从配置文件读取
        let config = DirectLlmClientConfig::new(LlmProvider::OpenClaw, None::<String>);
        assert_eq!(config.provider, LlmProvider::OpenClaw);
        assert_eq!(config.api_key, None);
        assert_eq!(config.base_url, None);
    }

    #[test]
    fn test_direct_client_openai_compatible_missing_fields() {
        // 缺少 base_url 和 model
        assert!(
            DirectLlmClient::new(DirectLlmClientConfig::new(
                LlmProvider::OpenAICompatible,
                Some("test-key")
            ))
            .is_err()
        );
    }

    #[test]
    fn test_direct_client_ollama() {
        let client = DirectLlmClient::ollama(None::<String>).unwrap();
        assert_eq!(client.config.provider, LlmProvider::Ollama);
        assert_eq!(client.config.api_key, None);
        assert_eq!(client.config.base_url, None); // 使用默认

        let client = DirectLlmClient::ollama(Some("http://localhost:11434/v1")).unwrap();
        assert_eq!(
            client.config.base_url,
            Some("http://localhost:11434/v1".to_string())
        );
    }

    #[test]
    fn test_temperature_clamping() {
        let config = DirectLlmClientConfig::new(LlmProvider::OpenClaw, Some("test-key"))
            .with_temperature(-0.5);
        assert_eq!(config.temperature, 0.0);

        let config = DirectLlmClientConfig::new(LlmProvider::OpenClaw, Some("test-key"))
            .with_temperature(1.5);
        assert_eq!(config.temperature, 1.0);
    }
}
