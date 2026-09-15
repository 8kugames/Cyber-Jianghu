// ============================================================================
// Direct LLM 客户端（模块根）
// ============================================================================
//
// 直接调用 LLM Provider API 的具体实现。模块划分：
// - openclaw:   OpenClaw 配置文件读取
// - provider:   LlmProvider 枚举（OpenClaw / OpenAI Compatible / Ollama）
// - config:     DirectLlmClientConfig + PromptConfig（builder）
// - http:       HTTP 传输层（send_request / via_stream / streaming）
// - 本文件:     全局开关、DirectLlmClient 本体、构造器、高层调用入口、LlmClient 实现
// ============================================================================

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::debug;

mod config;
mod http;
mod openclaw;
mod provider;

pub use config::{DirectLlmClientConfig, PromptConfig};
pub use openclaw::OpenClawConfig;
pub use provider::LlmProvider;

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
use super::openai_types::{ChatMessage, OpenAIRequest};
use super::tool_types::{ToolDefinition, ToolExecutor};

/// Direct LLM 客户端
///
/// 直接调用 LLM Provider API
#[derive(Debug)]
pub struct DirectLlmClient {
    config: DirectLlmClientConfig,
    earth_soul_config: Option<crate::soul::earth::config::EarthSoulConfig>,
    /// 最近一次 LLM 调用的 reasoning_content（DeepSeek 等模型需要回传）
    last_reasoning_content: std::sync::Mutex<Option<String>>,
    /// 最近一次 tool loop 的 tool call 日志
    last_tool_call_log: std::sync::Mutex<Option<Vec<cyber_jianghu_protocol::EarthToolCall>>>,
    /// 已见过的 system_hash 集合（用于检测 prefix cache 失效）。
    /// actor/validator 等调用类型共享本 client 并交替使用不同 prompt，
    /// 只比较"上一次"hash 会在合法交替时持续误报；集合语义下仅全新 hash 告警。
    known_system_hashes: std::sync::Mutex<std::collections::HashSet<[u8; 32]>>,
    /// 共享 circuit-breaker：由 FallbackLlmClient 注入，保证
    /// `run_tool_loop` 内部 send_chat_exchange 也走同一份禁用表
    breaker: Option<std::sync::Arc<super::client::SharedBreaker>>,
}

impl Clone for DirectLlmClient {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            earth_soul_config: self.earth_soul_config.clone(),
            last_reasoning_content: std::sync::Mutex::new(None),
            last_tool_call_log: std::sync::Mutex::new(None),
            known_system_hashes: std::sync::Mutex::new(std::collections::HashSet::new()),
            breaker: self.breaker.clone(),
        }
    }
}

/// 已知 system hash 集合容量。稳定运行时每 client 仅数种 prompt 类型
/// （actor/validator/triage/summary），64 足够；超限重置避免无界增长
/// （代价是重置后首个 hash 误报一次，可接受）。
const KNOWN_SYSTEM_HASHES_MAX: usize = 64;

/// 记录一次 system hash 观测。返回 true 当且仅当该 hash 从未见过（应告警）。
fn track_system_hash(known: &mut std::collections::HashSet<[u8; 32]>, hash: [u8; 32]) -> bool {
    if known.len() >= KNOWN_SYSTEM_HASHES_MAX {
        known.clear();
    }
    known.insert(hash)
}

/// 将字节偏移回退到最近的 UTF-8 char 边界（防切片 panic）
fn utf8_safe_end(s: &str, end: usize) -> usize {
    let mut e = end.min(s.len());
    while e > 0 && !s.is_char_boundary(e) {
        e -= 1;
    }
    e
}

impl DirectLlmClient {
    /// 读取内部 LLM 客户端配置（只读）。
    /// 测试需要断言 LlmConfig 字段是否被端到端传到这里。
    pub fn config(&self) -> &DirectLlmClientConfig {
        &self.config
    }

    /// 创建新的 Direct LLM 客户端
    pub fn new(mut config: DirectLlmClientConfig) -> Result<Self> {
        // 对于 OpenClaw，自动加载配置文件
        if config.provider == LlmProvider::OpenClaw {
            config = config.load_from_openclaw_config()?;
        }
        // 验证配置
        config.validate()?;
        Ok(Self {
            config,
            earth_soul_config: None,
            last_reasoning_content: std::sync::Mutex::new(None),
            last_tool_call_log: std::sync::Mutex::new(None),
            known_system_hashes: std::sync::Mutex::new(std::collections::HashSet::new()),
            breaker: None,
        })
    }

    /// 设置 EarthSoul 配置（由 AgentBuilder 调用）
    pub fn with_earth_soul_config(
        mut self,
        config: crate::soul::earth::config::EarthSoulConfig,
    ) -> Self {
        self.earth_soul_config = Some(config);
        self
    }

    /// 注入共享 circuit-breaker（由 FallbackLlmClient 调用）
    pub fn with_breaker(mut self, breaker: std::sync::Arc<super::client::SharedBreaker>) -> Self {
        self.breaker = Some(breaker);
        self
    }

    /// 检查共享 breaker：命中则直接返回 Err，不发起 HTTP 请求
    fn check_breaker(&self) -> Result<()> {
        if let Some(breaker) = &self.breaker
            && let Some(remaining) = breaker.is_disabled(&self.breaker_key())
        {
            anyhow::bail!(
                "LLM model {}/{} is in cooldown ({}s remaining)",
                self.config.provider.as_str(),
                self.config.get_model_with_default(),
                remaining
            );
        }
        Ok(())
    }

    /// 生成 breaker key：provider + model 联合标识
    fn breaker_key(&self) -> String {
        format!(
            "{}/{}",
            self.config.provider.as_str(),
            self.config.get_model_with_default()
        )
    }

    /// 获取当前使用的模型名称
    pub fn take_last_reasoning_content(&self) -> Option<String> {
        self.last_reasoning_content
            .lock()
            .ok()
            .and_then(|mut g| g.take())
    }

    pub fn take_last_tool_call_log(&self) -> Option<Vec<cyber_jianghu_protocol::EarthToolCall>> {
        self.last_tool_call_log
            .lock()
            .ok()
            .and_then(|mut g| g.take())
    }

    fn save_reasoning_content(&self, rc: Option<String>) {
        if let Ok(mut g) = self.last_reasoning_content.lock() {
            *g = rc;
        }
    }

    fn save_tool_call_log(&self, log: Vec<cyber_jianghu_protocol::EarthToolCall>) {
        if !log.is_empty()
            && let Ok(mut g) = self.last_tool_call_log.lock()
        {
            *g = Some(log);
        }
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
        // 超时从 config 消费，替代之前硬编码 120s。
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                self.config.request_timeout_secs,
            ))
            .connect_timeout(std::time::Duration::from_secs(
                self.config.connect_timeout_secs,
            ))
            .build()
            .context("Failed to build HTTP client")
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
            enable_thinking: self.config.enable_thinking,
            stream: None,
            stream_options: None,
        };
        self.send_streaming_request(&request).await
    }

    /// 流式对话完成（长窗口）
    pub async fn complete_conversation_streaming(
        &self,
        system: &str,
        semi_static: &str,
        summary: Option<&str>,
        turns: &[super::client::ConversationTurn],
        current_prompt: &str,
    ) -> Result<super::streaming::LlmStream> {
        let messages = super::client::build_conversation_messages(
            system,
            semi_static,
            summary,
            turns,
            current_prompt,
            self.config.prompt.strip_reasoning_content,
        );
        let model = self.config.get_model_with_default();
        let request = OpenAIRequest {
            model,
            messages,
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            tools: None,
            tool_choice: None,
            enable_thinking: self.config.enable_thinking,
            stream: None,
            stream_options: None,
        };
        self.send_streaming_request(&request).await
    }

    /// 构造无工具、非流式的 OpenAI 兼容请求（消息列表由调用方决定）
    fn plain_request(&self, messages: Vec<ChatMessage>) -> OpenAIRequest {
        OpenAIRequest {
            model: self.config.get_model_with_default(),
            messages,
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            tools: None,
            tool_choice: None,
            enable_thinking: self.config.enable_thinking,
            stream: None,
            stream_options: None,
        }
    }

    /// 发送请求并提取 first choice 文本（空白/缺失统一报错）
    async fn send_and_extract(&self, messages: Vec<ChatMessage>, label: &str) -> Result<String> {
        let request = self.plain_request(messages);
        let response_data = self.send_request(&request).await?;

        let Some(choice) = response_data.choices.first() else {
            anyhow::bail!("LLM returned empty response");
        };
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
        debug!("LLM {} response: {} chars", label, content.len());
        Ok(content)
    }

    /// 调用 OpenAI 兼容 API
    ///
    /// OpenClaw Gateway、OpenAI Compatible、Ollama 都使用 OpenAI 兼容接口
    async fn call_openai_compatible_api(&self, prompt: &str) -> Result<String> {
        self.send_and_extract(vec![ChatMessage::user(prompt)], "completion")
            .await
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
        debug!("Calling OpenAI-compatible API (system+user)");
        self.send_and_extract(
            vec![ChatMessage::system(system), ChatMessage::user(prompt)],
            "system+user",
        )
        .await
    }

    /// 使用对话历史完成调用（长窗口）
    ///
    /// 构建 system + semi-static + summary + 历史轮次 + 当前 prompt 的完整消息列表。
    async fn call_with_conversation(
        &self,
        system: &str,
        semi_static: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<String> {
        let messages = super::client::build_conversation_messages(
            system,
            semi_static,
            summary,
            turns,
            current_prompt,
            self.config.prompt.strip_reasoning_content,
        );

        debug!(
            "LLM conversation call: {} history turns, prompt_len={}",
            turns.len(),
            current_prompt.len(),
        );

        self.send_and_extract(messages, "conversation").await
    }
}

/// 按请求字符长度估算 prompt tokens（中文 ~1.5 char/token，英文 ~4，取中间值 3）
fn estimate_prompt_tokens(request: &OpenAIRequest) -> u64 {
    let prompt_chars: usize = request
        .messages
        .iter()
        .filter_map(|m| m.content.as_ref().map(|c| c.len()))
        .sum();
    (prompt_chars as u64 / 3).max(1)
}

#[allow(private_interfaces)]
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
        semi_static: &str,
        summary: Option<&str>,
        turns: &[ConversationTurn],
        current_prompt: &str,
    ) -> Result<String> {
        if is_llm_disabled() {
            anyhow::bail!("LLM 调用已被停止");
        }
        self.call_with_conversation(system, semi_static, summary, turns, current_prompt)
            .await
    }

    fn supports_tool_calling(&self) -> bool {
        true
    }

    fn context_window_tokens(&self) -> u32 {
        self.config.context_window_tokens
    }

    fn retry_max_tokens_baseline(&self) -> u32 {
        self.config.max_tokens
    }

    fn retry_max_tokens_ceiling(&self) -> u32 {
        self.config.context_window_tokens
    }

    fn temperature(&self) -> f32 {
        self.config.temperature
    }

    async fn send_chat_exchange(
        &self,
        messages: Vec<ChatMessage>,
        tools: Option<&[ToolDefinition]>,
        config: super::openai_types::ChatExchangeConfig,
    ) -> Result<super::openai_types::ChatExchangeResponse> {
        // 共享 breaker 守门：模型在冷却期直接拒绝
        self.check_breaker()?;

        let request = OpenAIRequest {
            model: config.model,
            messages,
            temperature: Some(config.temperature),
            max_tokens: config.max_tokens.or(Some(self.config.max_tokens)),
            tools: tools.map(|t| {
                t.iter()
                    .map(|tool| {
                        if self.config.prompt.canonicalize_schemas {
                            serde_json::from_str(&tool.canonical_json()).unwrap_or_else(|_| {
                                serde_json::to_value(tool).unwrap_or(serde_json::Value::Null)
                            })
                        } else {
                            serde_json::to_value(tool).unwrap_or(serde_json::Value::Null)
                        }
                    })
                    .collect()
            }),
            tool_choice: tools.and(Some(serde_json::json!("auto"))),
            enable_thinking: config.enable_thinking,
            stream: None,
            stream_options: None,
        };
        let response = self.send_request(&request).await?;
        let choice = response
            .choices
            .first()
            .ok_or_else(|| anyhow::anyhow!("LLM returned empty response"))?;
        Ok(super::openai_types::ChatExchangeResponse {
            content: choice.message.content.clone(),
            tool_calls: choice.message.tool_calls.clone(),
            reasoning_content: choice.message.reasoning_content.clone(),
        })
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

    fn take_last_reasoning_content(&self) -> Option<String> {
        self.last_reasoning_content
            .lock()
            .ok()
            .and_then(|mut g| g.take())
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
        let messages = vec![ChatMessage::system(system), ChatMessage::user(prompt)];
        let config = super::openai_types::ChatExchangeConfig {
            model: self.config.get_model_with_default(),
            temperature: self.config.temperature,
            max_tokens: Some(self.config.max_tokens),
            enable_thinking: self.config.enable_thinking,
        };
        let result = crate::soul::earth::tool_loop::run_tool_loop(
            self,
            messages,
            tools,
            executor,
            max_rounds,
            self.earth_soul_config.as_ref(),
            config,
        )
        .await?;
        self.save_reasoning_content(result.reasoning_content);
        self.save_tool_call_log(result.tool_call_log);
        Ok(result.content)
    }

    fn take_last_tool_call_log(&self) -> Option<Vec<cyber_jianghu_protocol::EarthToolCall>> {
        self.last_tool_call_log
            .lock()
            .ok()
            .and_then(|mut g| g.take())
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
        // 不使用 build_conversation_messages：tool loop 需要纯 history+current，
        // system 和 semi-static 由 tool_loop 自己管理
        //
        // 通用逻辑：合并 persona + semi_static + summary 为单个 system message。
        // OpenAI 规范对连续 system 消息无定义，部分 provider（如 sensenova）拒绝。
        // 对模型而言信息量等价 → 合并无损且更安全。
        let mut combined_system =
            String::with_capacity(system.len() + input.semi_static.len() + 64);
        combined_system.push_str(system);
        if !input.semi_static.is_empty() {
            combined_system.push_str("\n\n");
            combined_system.push_str(input.semi_static);
        }
        if let Some(s) = input.summary {
            combined_system.push_str("\n\n## 对话历史摘要\n");
            combined_system.push_str(s);
        }

        let mut messages = vec![ChatMessage::system(&combined_system)];
        for turn in input.turns {
            messages.push(ChatMessage::user(&turn.user));
            messages.push(ChatMessage::assistant_with_reasoning(
                &turn.assistant,
                if self.config.prompt.strip_reasoning_content {
                    None
                } else {
                    turn.reasoning_content.clone()
                },
            ));
        }
        messages.push(ChatMessage::user(input.current_prompt));
        let config = super::openai_types::ChatExchangeConfig {
            model: self.config.get_model_with_default(),
            temperature: self.config.temperature,
            max_tokens: Some(self.config.max_tokens),
            enable_thinking: self.config.enable_thinking,
        };
        let result = crate::soul::earth::tool_loop::run_tool_loop(
            self,
            messages,
            tools,
            executor,
            max_rounds,
            self.earth_soul_config.as_ref(),
            config,
        )
        .await?;
        self.save_reasoning_content(result.reasoning_content);
        self.save_tool_call_log(result.tool_call_log);
        Ok(result.content)
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
            let prompt_chars = super::streaming::plain_prompt_chars(system, prompt);
            let system_hash = crate::soul::actor::compute_system_hash(system);
            let stream = self.complete_streaming(system, prompt).await?;
            Ok(super::streaming::wrap_usage_tracking(
                stream,
                self.config.provider,
                self.config.get_model_with_default(),
                system_hash,
                prompt_chars,
            ))
        })
    }

    fn complete_conversation_streaming<'a>(
        &'a self,
        system: &'a str,
        semi_static: &'a str,
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
            let prompt_chars = super::streaming::conversation_prompt_chars(
                system,
                semi_static,
                summary,
                turns,
                current_prompt,
            );
            let stream = self
                .complete_conversation_streaming(
                    system,
                    semi_static,
                    summary,
                    turns,
                    current_prompt,
                )
                .await?;
            let system_hash = crate::soul::actor::compute_system_hash(system);
            Ok(super::streaming::wrap_usage_tracking(
                stream,
                self.config.provider,
                self.config.get_model_with_default(),
                system_hash,
                prompt_chars,
            ))
        })
    }
}

// ============================================================================
// Tests

#[cfg(test)]
#[path = "direct_client/direct_client_tests.rs"]
mod tests;
