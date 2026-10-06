// ============================================================================
// DirectLlmClient 配置（provider/model/temperature/max_tokens/prompt 策略 + builder）
// ============================================================================

use super::openclaw::OpenClawConfig;
use super::provider::LlmProvider;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tracing::debug;

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
    /// 优先使用流式调用（避免对只支持 streaming 的模型浪费 400 降级）
    /// 使用 Arc<AtomicBool> 实现 sticky 自动翻转：首次 400+"stream" 降级后设为 true
    pub prefer_stream: Arc<AtomicBool>,
    /// DashScope/Kimi 等模型的 enable_thinking 参数（None = 不发送该字段）
    pub enable_thinking: Option<bool>,
    /// 上下文窗口大小（tokens）
    pub context_window_tokens: u32,
    /// Prompt 配置（D8 reasoning 剥离 + D9 schema 规范化开关）
    pub prompt: PromptConfig,
    /// HTTP 请求整体超时（与 Server LlmConfig.request_timeout_secs 对齐，默认 120s）
    pub request_timeout_secs: u64,
    /// HTTP 连接超时（与 Server LlmConfig.connect_timeout_secs 对齐，默认 30s）
    pub connect_timeout_secs: u64,
}

/// Prompt 配置（D8 reasoning 剥离 + D9 schema 规范化开关）
#[derive(Debug, Clone)]
pub struct PromptConfig {
    /// 是否从 LLM 输出中剥离 reasoning content（D8）
    pub strip_reasoning_content: bool, // env var: CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT
    /// 是否规范化 tool/parameter schema 输出（D9）
    pub canonicalize_schemas: bool, // env var: CYBER_JIANGHU_PROMPT_CANONICALIZE_SCHEMAS
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            strip_reasoning_content: crate::config::env_or(
                "CYBER_JIANGHU_PROMPT_STRIP_REASONING_CONTENT",
                false,
            ),
            canonicalize_schemas: crate::config::env_or(
                "CYBER_JIANGHU_PROMPT_CANONICALIZE_SCHEMAS",
                true,
            ),
        }
    }
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
            max_tokens: crate::config::DEFAULT_LLM_MAX_TOKENS,
            prefer_stream: Arc::new(AtomicBool::new(false)),
            enable_thinking: None,
            context_window_tokens: 32768,
            prompt: PromptConfig::default(),
            request_timeout_secs: crate::config::DEFAULT_LLM_REQUEST_TIMEOUT_SECS,
            connect_timeout_secs: crate::config::DEFAULT_LLM_CONNECT_TIMEOUT_SECS,
        }
    }

    /// 覆盖 HTTP 请求整体超时（秒）。
    /// 设置过低会导致长上下文请求被截断；设置过高会让 cognitive retry 雪崩。
    pub fn with_request_timeout_secs(mut self, secs: u64) -> Self {
        self.request_timeout_secs = secs;
        self
    }

    /// 覆盖 HTTP 连接超时（秒）。
    pub fn with_connect_timeout_secs(mut self, secs: u64) -> Self {
        self.connect_timeout_secs = secs;
        self
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

    /// 设置 enable_thinking 参数（DashScope/Kimi 等模型需要）
    pub fn with_enable_thinking(mut self, enable_thinking: Option<bool>) -> Self {
        self.enable_thinking = enable_thinking;
        self
    }

    /// 设置上下文窗口大小
    pub fn with_context_window_tokens(mut self, tokens: u32) -> Self {
        self.context_window_tokens = tokens;
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
            self.provider
                .default_model()
                .expect("provider must have default model")
                .to_string()
        } else {
            "default".to_string()
        }
    }
}
