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
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, error, info};

/// 全局 LLM 停止标志
static LLM_DISABLED: AtomicBool = AtomicBool::new(false);

/// 检查 LLM 是否被禁用
pub fn is_llm_disabled() -> bool {
    LLM_DISABLED.load(Ordering::Relaxed)
}

/// 设置 LLM 停止状态
pub fn set_llm_disabled(disabled: bool) {
    LLM_DISABLED.store(disabled, Ordering::Relaxed);
}

use super::LlmClient;
use super::client::ConversationTurn;
use super::openai_types::{ChatMessage, OpenAIRequest, OpenAIResponse};
use super::token_tracking::record_token_usage;
use super::tool_types::{ToolDefinition, ToolExecutor};

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
    /// 优先使用流式调用（避免对只支持 streaming 的模型浪费 400 降级）
    pub prefer_stream: bool,
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
            prefer_stream: false,
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

    /// 根据模型名称返回 enable_thinking 参数
    ///
    /// - qwen3-thinking 系列：DashScope 强制要求 `enable_thinking: true`
    /// - 其他 qwen/kimi 系列：非流式调用必须 `enable_thinking: false`
    fn extra_body_for_model(model: &str) -> Option<bool> {
        let lower = model.to_ascii_lowercase();
        // qwen3-thinking 系列强制要求 enable_thinking=true
        if lower.contains("thinking") {
            Some(true)
        } else if lower.contains("kimi")
            || lower.contains("qwen")
            || lower.contains("qwq")
            || lower.contains("qvq")
        {
            Some(false)
        } else {
            None
        }
    }

    /// 发送 OpenAI 兼容 API 请求（公共 HTTP 逻辑）
    async fn send_request(&self, request: &OpenAIRequest) -> Result<OpenAIResponse> {
        // prefer_stream: 直接走流式，避免对只支持 streaming 的模型浪费 400 降级
        if self.config.prefer_stream {
            return self.send_request_via_stream(request).await;
        }

        let client = self.build_http_client()?;
        let base_url = self.config.get_base_url()?;
        let base_url = base_url.trim_end_matches('/');
        let url = if base_url.contains("/chat/completions") {
            base_url.to_string()
        } else {
            format!("{}/chat/completions", base_url)
        };

        debug!("Calling OpenAI-compatible API: {}", url);
        debug!("Request model: {}", request.model);
        // 地魂诊断：确认 tools 字段是否在请求中
        if request.tools.is_some() {
            info!(
                "[地魂] 发送请求: url={}, model={}, tools={}, tool_choice={}, stream={:?}, prefer_stream={}",
                url,
                request.model,
                request.tools.as_ref().map(|t| t.len()).unwrap_or(0),
                request
                    .tool_choice
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "None".to_string()),
                request.stream,
                self.config.prefer_stream,
            );
        }

        let mut request_builder = client.post(&url).header("Content-Type", "application/json");

        if let Some(ref api_key) = self.config.api_key {
            request_builder =
                request_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        // 网络错误时立即重试一次（无等待，避免积压）
        let response = match request_builder.try_clone() {
            Some(rb1) => match rb1.json(&request).send().await {
                Ok(r) => r,
                Err(e) if e.is_connect() || e.is_timeout() || e.is_request() => {
                    tracing::warn!("LLM 请求发送失败（网络错误），立即重试一次: {}", e);
                    request_builder
                        .json(&request)
                        .send()
                        .await
                        .context("LLM API request failed after 1 retry")?
                }
                Err(e) => return Err(e).context("Failed to send request to LLM API"),
            },
            None => {
                // 无法 clone，直接请求
                request_builder
                    .json(&request)
                    .send()
                    .await
                    .context("Failed to send request to LLM API")?
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());
            error!("LLM API error {}: {}", status, error_body);

            // 400 + "stream"：模型强制要求流式，自动用流式重试
            if status.as_u16() == 400 && error_body.contains("stream") {
                info!("模型要求流式调用，自动切换到 streaming 重试");
                return self.send_request_via_stream(request).await;
            }

            super::token_tracking::record_failure(
                &self.config.provider,
                &self.config.get_model_with_default(),
            );
            anyhow::bail!("LLM API error {}: {}", status, error_body);
        }

        // DEBUG: 工具调用时打印原始响应 body 的 tool_calls 部分
        let raw_body = response
            .text()
            .await
            .context("Failed to read response body")?;
        if request.tools.is_some() {
            let tool_calls_preview = if let Some(tc_start) = raw_body.find("\"tool_calls\"") {
                &raw_body[tc_start..raw_body.len().min(tc_start + 300)]
            } else {
                "tool_calls field NOT FOUND in response"
            };
            debug!(
                "[地魂] 原始 API 响应 (tool_calls 片段): {}",
                tool_calls_preview
            );
        }
        let response_data: OpenAIResponse = serde_json::from_str(&raw_body).map_err(|e| {
            super::token_tracking::record_failure(
                &self.config.provider,
                &self.config.get_model_with_default(),
            );
            anyhow::anyhow!("Failed to parse LLM response: {}", e)
        })?;

        let model = self.config.get_model_with_default();
        if let Some(ref actual_model) = response_data.model
            && actual_model != &model
        {
            info!(
                "[llm] model fallback: requested={}, actual={}",
                model, actual_model
            );
        }
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
        } else {
            // API 未返回 usage，按字符长度估算（中文 ~1.5 char/token，英文 ~4 char/token，取中间值 3）
            let prompt_chars: usize = request
                .messages
                .iter()
                .filter_map(|m| m.content.as_ref().map(|c| c.len()))
                .sum();
            let est_pt = (prompt_chars as u64 / 3).max(1);
            let est_ct = response_data
                .choices
                .first()
                .and_then(|c| c.message.content.as_ref())
                .map(|s| (s.len() as u64 / 3).max(1))
                .unwrap_or(0);
            record_token_usage(&self.config.provider, &model, est_pt, est_ct);
            debug!(
                "Token usage (estimated): provider={}, model={}, prompt~{}, completion~{}",
                self.config.provider.as_str(),
                model,
                est_pt,
                est_ct
            );
        }

        Ok(response_data)
    }

    /// 流式降级：用 streaming 收集完整响应，组装为 OpenAIResponse
    ///
    /// 当 send_request 遇到 "only support stream mode" 错误时调用此方法。
    /// 复用 send_streaming_request 建立 SSE 连接，收集全部 Delta 后拼装响应。
    async fn send_request_via_stream(&self, request: &OpenAIRequest) -> Result<OpenAIResponse> {
        use super::streaming::StreamAccumulator;
        use futures_util::StreamExt;

        let mut stream = self.send_streaming_request(request).await?;
        let mut acc = StreamAccumulator::new();

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(c) => acc.push(c),
                Err(e) => {
                    tracing::warn!("流式降级收集中途失败: {}", e);
                    break;
                }
            }
        }

        let (pt, ct, has_real) = acc.token_stats();
        let content = acc.into_content();

        // 记录流式 token 用量
        if pt > 0 || ct > 0 {
            record_token_usage(
                &self.config.provider,
                &self.config.get_model_with_default(),
                pt,
                ct,
            );
            debug!(
                "Stream token usage: provider={}, model={}, prompt={}, completion={}, real_usage={}",
                self.config.provider.as_str(),
                self.config.get_model_with_default(),
                pt,
                ct,
                has_real
            );
        } else if !has_real {
            // 服务端未返回 usage，使用估算值
            let est_ct = (content.len() as u64 / 3).max(1);
            record_token_usage(
                &self.config.provider,
                &self.config.get_model_with_default(),
                0,
                est_ct,
            );
            debug!(
                "Stream token usage (estimated): provider={}, model={}, prompt=0, completion={}",
                self.config.provider.as_str(),
                self.config.get_model_with_default(),
                est_ct
            );
        }

        // 组装为 OpenAIResponse 格式（与 send_request 返回一致）
        Ok(OpenAIResponse {
            choices: vec![super::openai_types::OpenAIChoice {
                message: super::openai_types::ChatMessage {
                    role: "assistant".to_string(),
                    content: Some(content),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
            }],
            usage: None,
            model: None,
        })
    }

    /// 发送流式请求到 OpenAI 兼容 API
    ///
    /// 返回 SSE 流，每个 chunk 为 StreamChunk::Delta 或 StreamChunk::Done
    async fn send_streaming_request(
        &self,
        request: &OpenAIRequest,
    ) -> Result<super::streaming::LlmStream> {
        let client = self.build_http_client()?;
        let base_url = self.config.get_base_url()?;
        let url = format!("{}/chat/completions", base_url);

        let mut request_builder = client.post(&url).header("Content-Type", "application/json");
        if let Some(ref api_key) = self.config.api_key {
            request_builder =
                request_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        // 设置 stream: true 和 stream_options: {"include_usage": true}
        // 这使得服务端在流式响应的最后一块返回 usage 数据
        let mut stream_request = request.clone();
        stream_request.stream = Some(true);
        stream_request.stream_options = Some(serde_json::json!({"include_usage": true}));

        // 网络错误时立即重试一次（无等待，避免积压）
        let response = match request_builder.try_clone() {
            Some(rb1) => match rb1.json(&stream_request).send().await {
                Ok(r) => r,
                Err(e) if e.is_connect() || e.is_timeout() || e.is_request() => {
                    tracing::warn!(
                        "LLM streaming 请求发送失败（网络错误），立即重试一次: {}",
                        e
                    );
                    request_builder
                        .json(&stream_request)
                        .send()
                        .await
                        .context("LLM streaming API request failed after 1 retry")?
                }
                Err(e) => return Err(e).context("Failed to send request to LLM streaming API"),
            },
            None => {
                // 无法 clone，直接请求
                request_builder
                    .json(&stream_request)
                    .send()
                    .await
                    .context("Failed to send request to LLM streaming API")?
            }
        };

        let status = response.status();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            super::token_tracking::record_failure(
                &self.config.provider,
                &self.config.get_model_with_default(),
            );
            anyhow::bail!("LLM streaming API error {}: {}", status, error_body);
        }

        debug!(
            "LLM streaming connection established: provider={}, model={}",
            self.config.provider.as_str(),
            self.config.get_model_with_default(),
        );

        Ok(super::streaming::parse_sse_stream(response))
    }

    /// 流式完成（system + user）
    pub async fn complete_streaming(
        &self,
        system: &str,
        prompt: &str,
    ) -> Result<super::streaming::LlmStream> {
        let model = self.config.get_model_with_default();
        let request = OpenAIRequest {
            model,
            messages: vec![ChatMessage::system(system), ChatMessage::user(prompt)],
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            tools: None,
            tool_choice: None,
            enable_thinking: Self::extra_body_for_model(&self.config.get_model_with_default()),
            stream: None,
            stream_options: None,
        };
        self.send_streaming_request(&request).await
    }

    /// 流式对话完成（长窗口）
    pub async fn complete_conversation_streaming(
        &self,
        system: &str,
        summary: Option<&str>,
        turns: &[super::client::ConversationTurn],
        current_prompt: &str,
    ) -> Result<super::streaming::LlmStream> {
        let messages =
            super::client::build_conversation_messages(system, summary, turns, current_prompt);
        let model = self.config.get_model_with_default();
        let request = OpenAIRequest {
            model,
            messages,
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            tools: None,
            tool_choice: None,
            enable_thinking: Self::extra_body_for_model(&self.config.get_model_with_default()),
            stream: None,
            stream_options: None,
        };
        self.send_streaming_request(&request).await
    }

    /// 调用 OpenAI 兼容 API
    ///
    /// OpenClaw Gateway、OpenAI Compatible、Ollama 都使用 OpenAI 兼容接口
    async fn call_openai_compatible_api(&self, prompt: &str) -> Result<String> {
        let model = self.config.get_model_with_default();
        let request = OpenAIRequest {
            model,
            messages: vec![ChatMessage::user(prompt)],
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            tools: None,
            tool_choice: None,
            enable_thinking: Self::extra_body_for_model(&self.config.get_model_with_default()),
            stream: None,
            stream_options: None,
        };

        let response_data = self.send_request(&request).await?;

        if let Some(choice) = response_data.choices.first() {
            let content = choice
                .message
                .content
                .clone()
                .unwrap_or_default()
                .trim()
                .to_string();
            if content.is_empty() {
                anyhow::bail!(
                    "LLM API error: response content is empty (model may have returned null/whitespace)"
                );
            }
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
        let model = self.config.get_model_with_default();
        let request = OpenAIRequest {
            model,
            messages: vec![ChatMessage::system(system), ChatMessage::user(prompt)],
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            tools: None,
            tool_choice: None,
            enable_thinking: Self::extra_body_for_model(&self.config.get_model_with_default()),
            stream: None,
            stream_options: None,
        };

        debug!("Calling OpenAI-compatible API (system+user)");

        let response_data = self.send_request(&request).await?;

        if let Some(choice) = response_data.choices.first() {
            let content = choice
                .message
                .content
                .clone()
                .unwrap_or_default()
                .trim()
                .to_string();
            if content.is_empty() {
                anyhow::bail!(
                    "LLM API error: response content is empty (model may have returned null/whitespace)"
                );
            }
            debug!("LLM response length: {} chars", content.len());
            Ok(content)
        } else {
            anyhow::bail!("LLM returned empty response")
        }
    }

    /// 使用 tool calling 的多轮对话
    async fn call_openai_compatible_api_with_tools(
        &self,
        system: &str,
        prompt: &str,
        tools: &[ToolDefinition],
        executor: &dyn ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let messages = vec![ChatMessage::system(system), ChatMessage::user(prompt)];
        self.run_tool_loop(messages, tools, executor, max_rounds)
            .await
    }

    /// 使用对话历史 + tool calling 的组合调用
    async fn call_with_conversation_and_tools(
        &self,
        system: &str,
        input: super::client::ConversationInput<'_>,
        tools: &[ToolDefinition],
        executor: &dyn ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let messages = super::client::build_conversation_messages(
            system,
            input.summary,
            input.turns,
            input.current_prompt,
        );
        self.run_tool_loop(messages, tools, executor, max_rounds)
            .await
    }

    /// Tool-calling 循环核心逻辑
    ///
    /// 接收预构建的消息列表，执行多轮 tool-calling 直到 LLM 返回文本或超时。
    async fn run_tool_loop(
        &self,
        messages: Vec<ChatMessage>,
        tools: &[ToolDefinition],
        executor: &dyn ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let model = self.config.get_model_with_default();
        let mut messages = messages;

        for round in 0..max_rounds {
            if round == 0 {
                let tool_names: Vec<&str> =
                    tools.iter().map(|t| t.function.name.as_str()).collect();
                info!(
                    "[地魂] Tool loop 开始, tools={:?}, max_rounds={}",
                    tool_names, max_rounds
                );
            }

            let request = OpenAIRequest {
                model: model.clone(),
                messages: messages.clone(),
                temperature: Some(self.config.temperature),
                max_tokens: Some(self.config.max_tokens),
                tools: Some(tools.to_vec()),
                tool_choice: Some(serde_json::json!("auto")),
                enable_thinking: Self::extra_body_for_model(&model),
                stream: None,
                stream_options: None,
            };

            let response_data = self.send_request(&request).await?;

            let choice = response_data
                .choices
                .first()
                .ok_or_else(|| anyhow::anyhow!("LLM returned empty response"))?;
            let msg = &choice.message;

            // DEBUG: 检查 tool_calls 字段是否存在于原始响应
            debug!(
                "[地魂] API 响应: tool_calls={}, content_len={}, content_preview={}",
                msg.tool_calls
                    .as_ref()
                    .map(|tc| format!(
                        "{:?}",
                        tc.iter()
                            .map(|t| t.function.name.clone())
                            .collect::<Vec<_>>()
                    ))
                    .unwrap_or_else(|| "None".to_string()),
                msg.content.as_ref().map(|c| c.len()).unwrap_or(0),
                msg.content
                    .as_ref()
                    .map(|c| c.chars().take(100).collect::<String>())
                    .unwrap_or_default(),
            );

            let has_tool_calls = msg
                .tool_calls
                .as_ref()
                .map(|tc| !tc.is_empty())
                .unwrap_or(false);

            if !has_tool_calls {
                let content = msg.content.clone().unwrap_or_default();
                info!(
                    "[地魂] LLM 未调用任何 tool，直接返回文本 ({} chars)",
                    content.len()
                );
                return Ok(content);
            }

            let tool_calls = msg.tool_calls.as_ref().unwrap();
            let call_names: Vec<&str> = tool_calls
                .iter()
                .map(|tc| tc.function.name.as_str())
                .collect();
            info!(
                "[地魂] LLM 请求调用 {} 个 tool: {:?}",
                tool_calls.len(),
                call_names
            );

            messages.push(msg.clone());

            for tc in tool_calls {
                let args = tc.parse_arguments().unwrap_or(serde_json::json!({}));
                info!("[地魂] 执行 tool: {}({})", tc.function.name, args);
                let result = executor
                    .execute(&tc.function.name, &args)
                    .await
                    .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));

                info!("[地魂] Tool {} 结果: {}", tc.function.name, result);

                messages.push(ChatMessage::tool_result(
                    &tc.id,
                    &tc.function.name,
                    &result.to_string(),
                ));
            }
        }

        debug!("Tool calling reached max rounds ({})", max_rounds);
        anyhow::bail!("Tool calling exceeded max rounds ({})", max_rounds)
    }

    /// 使用对话历史完成调用（长窗口）
    ///
    /// 构建 system (含摘要) + 历史轮次 + 当前 prompt 的完整消息列表。
    async fn call_with_conversation(
        &self,
        system: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<String> {
        let messages =
            super::client::build_conversation_messages(system, summary, turns, current_prompt);

        let model = self.config.get_model_with_default();
        let request = OpenAIRequest {
            model,
            messages,
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            tools: None,
            tool_choice: None,
            enable_thinking: Self::extra_body_for_model(&self.config.get_model_with_default()),
            stream: None,
            stream_options: None,
        };

        debug!(
            "LLM conversation call: {} history turns, prompt_len={}",
            turns.len(),
            current_prompt.len(),
        );

        let response_data = self.send_request(&request).await?;

        if let Some(choice) = response_data.choices.first() {
            let content = choice
                .message
                .content
                .clone()
                .unwrap_or_default()
                .trim()
                .to_string();
            if content.is_empty() {
                anyhow::bail!("LLM API error: response content is empty");
            }
            debug!("LLM conversation response: {} chars", content.len());
            Ok(content)
        } else {
            anyhow::bail!("LLM returned empty response")
        }
    }
}

#[async_trait]
impl LlmClient for DirectLlmClient {
    async fn complete(&self, prompt: &str) -> Result<String> {
        if is_llm_disabled() {
            anyhow::bail!("LLM 调用已被停止");
        }
        // 所有三种 provider 都使用 OpenAI 兼容接口
        self.call_openai_compatible_api(prompt).await
    }

    async fn complete_with_system(&self, system: &str, prompt: &str) -> Result<String> {
        if is_llm_disabled() {
            anyhow::bail!("LLM 调用已被停止");
        }
        self.call_openai_compatible_api_with_system(system, prompt)
            .await
    }

    async fn complete_with_conversation(
        &self,
        system: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<String> {
        if is_llm_disabled() {
            anyhow::bail!("LLM 调用已被停止");
        }
        self.call_with_conversation(system, summary, turns, current_prompt)
            .await
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn provider_name(&self) -> String {
        self.config.provider.as_str().to_string()
    }

    fn model_name(&self) -> String {
        self.config.get_model_with_default()
    }

    fn provider_info(&self) -> (LlmProvider, String) {
        (self.config.provider, self.config.get_model_with_default())
    }

    async fn complete_with_tools(
        &self,
        system: &str,
        prompt: &str,
        tools: &[ToolDefinition],
        executor: &dyn ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        if is_llm_disabled() {
            anyhow::bail!("LLM 调用已被停止");
        }
        self.call_openai_compatible_api_with_tools(system, prompt, tools, executor, max_rounds)
            .await
    }

    async fn complete_with_conversation_and_tools(
        &self,
        system: &str,
        input: super::client::ConversationInput<'_>,
        tools: &[ToolDefinition],
        executor: &dyn ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        if is_llm_disabled() {
            anyhow::bail!("LLM 调用已被停止");
        }
        self.call_with_conversation_and_tools(system, input, tools, executor, max_rounds)
            .await
    }

    fn complete_streaming<'a>(
        &'a self,
        system: &'a str,
        prompt: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<super::streaming::LlmStream>> + Send + 'a>,
    > {
        Box::pin(async move {
            if is_llm_disabled() {
                anyhow::bail!("LLM 调用已被停止");
            }
            let stream = self.complete_streaming(system, prompt).await?;
            let tracking = super::streaming::UsageTrackingStream::new(
                stream,
                self.config.provider,
                self.config.get_model_with_default(),
            );
            Ok(tracking.into_llm_stream())
        })
    }

    fn complete_conversation_streaming<'a>(
        &'a self,
        system: &'a str,
        summary: Option<&'a str>,
        turns: &'a [ConversationTurn],
        current_prompt: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<super::streaming::LlmStream>> + Send + 'a>,
    > {
        Box::pin(async move {
            if is_llm_disabled() {
                anyhow::bail!("LLM 调用已被停止");
            }
            let stream = self
                .complete_conversation_streaming(system, summary, turns, current_prompt)
                .await?;
            let tracking = super::streaming::UsageTrackingStream::new(
                stream,
                self.config.provider,
                self.config.get_model_with_default(),
            );
            Ok(tracking.into_llm_stream())
        })
    }
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
