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
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, error};

use super::LlmClient;

// ============================================================================
// Token Usage Tracking
// ============================================================================

/// LLM Token 累计使用统计
///
/// 使用 AtomicU64 实现无锁并发累加，通过全局单例共享。
/// 适用于单进程 Agent 场景。
pub struct TokenUsageTracker {
    /// 累计 Prompt Tokens
    pub total_prompt_tokens: AtomicU64,
    /// 累计 Completion Tokens
    pub total_completion_tokens: AtomicU64,
    /// 累计 API 调用次数
    pub total_calls: AtomicU64,
}

impl TokenUsageTracker {
    fn new() -> Self {
        Self {
            total_prompt_tokens: AtomicU64::new(0),
            total_completion_tokens: AtomicU64::new(0),
            total_calls: AtomicU64::new(0),
        }
    }

    /// 记录一次 API 调用的 token 使用量
    pub fn record(&self, prompt_tokens: u64, completion_tokens: u64) {
        self.total_prompt_tokens
            .fetch_add(prompt_tokens, Ordering::Relaxed);
        self.total_completion_tokens
            .fetch_add(completion_tokens, Ordering::Relaxed);
        self.total_calls.fetch_add(1, Ordering::Relaxed);
    }

    /// 获取当前统计数据
    pub fn snapshot(&self) -> TokenUsageSnapshot {
        let prompt = self.total_prompt_tokens.load(Ordering::Relaxed);
        let completion = self.total_completion_tokens.load(Ordering::Relaxed);
        TokenUsageSnapshot {
            total_prompt_tokens: prompt,
            total_completion_tokens: completion,
            total_tokens: prompt + completion,
            total_calls: self.total_calls.load(Ordering::Relaxed),
        }
    }
}

/// Token 使用统计快照
#[derive(Debug, Clone, Serialize)]
pub struct TokenUsageSnapshot {
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_tokens: u64,
    pub total_calls: u64,
}

static TOKEN_USAGE: OnceLock<TokenUsageTracker> = OnceLock::new();

/// 获取全局 Token 使用统计追踪器
pub fn token_usage_tracker() -> &'static TokenUsageTracker {
    TOKEN_USAGE.get_or_init(TokenUsageTracker::new)
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
            model,
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
            token_usage_tracker().record(usage.prompt_tokens, usage.completion_tokens);
            debug!(
                "Token usage: prompt={}, completion={}",
                usage.prompt_tokens, usage.completion_tokens
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
    fn test_direct_client_openclaw_with_url() {
        let client = DirectLlmClient::openclaw_with_url("http://custom:9999").unwrap();
        assert_eq!(client.config.provider, LlmProvider::OpenClaw);
        assert_eq!(
            client.config.base_url,
            Some("http://custom:9999".to_string())
        );
        assert_eq!(client.config.api_key, None);
    }

    #[test]
    fn test_direct_client_openai_compatible() {
        let client =
            DirectLlmClient::openai_compatible("https://api.example.com/v1", "gpt-4", "test-key")
                .unwrap();
        assert_eq!(client.config.provider, LlmProvider::OpenAICompatible);
        assert_eq!(
            client.config.base_url,
            Some("https://api.example.com/v1".to_string())
        );
        assert_eq!(client.config.model, Some("gpt-4".to_string()));
        assert_eq!(client.config.api_key, Some("test-key".to_string()));
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

        let client = DirectLlmClient::ollama(Some("http://custom:11434/v1")).unwrap();
        assert_eq!(
            client.config.base_url,
            Some("http://custom:11434/v1".to_string())
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
