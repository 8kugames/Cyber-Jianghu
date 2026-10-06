// ============================================================================
// 主/从双链场景分发客户端（RoutedLlmClient）
// ============================================================================
//
// 场景级成本分流的客户端层实现：按 task-local 场景标签（component::llm::scenario）
// 将请求分发到主链（FallbackLlmClient，来自 Config.llm）或从链（来自
// Config.llm_secondary）。从模型可跨 Provider（独立 base_url/api_key/model）。
//
// 分发语义：
// - 场景显式配置（llm.scenario_routing）优先；未配置按内置默认
//   （轻量场景走从，scenario::defaults_to_secondary）；其余走主。
// - llm_secondary 未配置（与主一致）时构建方不包装本客户端（从即主）。
// - 从链调用失败时自动回退主链一次（对齐零风险灰度哲学：从模型故障
//   只多花钱不丢功能）；主/从链各自保留内部重试与降级链。
// - send_chat_exchange 分发时将 config.model 归一化为目标链的实际模型名，
//   根治跨链分发的陈旧模型名问题（主链模型名打到从链 provider）。
// - 记账天然准确：token 记账发生在各 DirectLlmClient 内部，按实际
//   provider/model + task-local 场景落盘，主/从各自独立条目。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use tracing::warn;

use super::client::ConversationInput;
use super::client::LlmClient;
use super::openai_types::{ChatExchangeConfig, ChatExchangeResponse, ChatMessage};
use super::scenario;
use super::tool_types::{ToolDefinition, ToolExecutor};
use crate::config::{ScenarioRouteConfig, ScenarioVia};

pub struct RoutedLlmClient {
    primary: Arc<dyn LlmClient>,
    secondary: Option<Arc<dyn LlmClient>>,
    routing: HashMap<String, ScenarioRouteConfig>,
    /// 最近一次分发端：take_last_* 系列调用后读取（此时场景 scope 可能已退出）
    last_via: Mutex<ScenarioVia>,
}

/// 是否全局停止导致的拒绝（非从模型故障）：回退时不打「从模型失败」误导日志
fn is_stop_bail(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains("LLM 调用已被停止")
}

impl RoutedLlmClient {
    pub fn new(
        primary: Arc<dyn LlmClient>,
        secondary: Option<Arc<dyn LlmClient>>,
        routing: HashMap<String, ScenarioRouteConfig>,
    ) -> Self {
        Self {
            primary,
            secondary,
            routing,
            last_via: Mutex::new(ScenarioVia::Primary),
        }
    }

    /// 解析当前场景的目标端与可选输出上限。
    /// 显式配置优先；未配置按内置默认（轻量场景走从）；无可用从链时回主。
    fn pick(&self) -> (ScenarioVia, Option<u32>) {
        let sc = scenario::current().0;
        let route = self.routing.get(sc);
        let via = match route {
            Some(r) => r.via,
            None if scenario::defaults_to_secondary(sc) => ScenarioVia::Secondary,
            None => ScenarioVia::Primary,
        };
        let via = if via == ScenarioVia::Secondary
            && (self.secondary.is_none() || super::direct_client::is_secondary_disabled())
        {
            ScenarioVia::Primary
        } else {
            via
        };
        (via, route.and_then(|r| r.max_tokens))
    }

    fn target(&self, via: ScenarioVia) -> Arc<dyn LlmClient> {
        if via == ScenarioVia::Secondary
            && let Some(ref secondary) = self.secondary
        {
            return secondary.clone();
        }
        self.primary.clone()
    }

    fn remember(&self, via: ScenarioVia) {
        if let Ok(mut guard) = self.last_via.lock() {
            *guard = via;
        }
    }

    /// 状态访问器（take_last_*）转发目标：按最近使用端
    fn state_client(&self) -> Arc<dyn LlmClient> {
        let via = self
            .last_via
            .lock()
            .map(|g| *g)
            .unwrap_or(ScenarioVia::Primary);
        self.target(via)
    }
}

#[async_trait]
impl LlmClient for RoutedLlmClient {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let (via, _) = self.pick();
        self.remember(via);
        let client = self.target(via);
        match client.complete(prompt).await {
            Ok(v) => Ok(v),
            Err(e) if via == ScenarioVia::Secondary => {
                if !is_stop_bail(&e) {
                    warn!("[llm] 从模型 complete 失败，回退主模型: {e:#}");
                }
                self.remember(ScenarioVia::Primary);
                self.primary.complete(prompt).await
            }
            Err(e) => Err(e),
        }
    }

    async fn complete_with_system(&self, system: &str, prompt: &str) -> Result<String> {
        let (via, _) = self.pick();
        self.remember(via);
        let client = self.target(via);
        match client.complete_with_system(system, prompt).await {
            Ok(v) => Ok(v),
            Err(e) if via == ScenarioVia::Secondary => {
                if !is_stop_bail(&e) {
                    warn!("[llm] 从模型 complete_with_system 失败，回退主模型: {e:#}");
                }
                self.remember(ScenarioVia::Primary);
                self.primary.complete_with_system(system, prompt).await
            }
            Err(e) => Err(e),
        }
    }

    async fn send_chat_exchange(
        &self,
        messages: Vec<ChatMessage>,
        tools: Option<&[ToolDefinition]>,
        mut config: ChatExchangeConfig,
    ) -> Result<ChatExchangeResponse> {
        let (via, cap) = self.pick();
        self.remember(via);
        let client = self.target(via);
        // 归一化：config.model 由调用方按其客户端默认构建，跨链分发时是陈旧值
        config.model = client.model_name();
        if let Some(cap) = cap {
            config.max_tokens = Some(cap);
        }
        // 从链路径预留克隆供失败回退（主链路径不付这笔小成本）
        let (retry_messages, retry_config) = if via == ScenarioVia::Secondary {
            (Some(messages.clone()), Some(config.clone()))
        } else {
            (None, None)
        };
        match client.send_chat_exchange(messages, tools, config).await {
            Ok(v) => Ok(v),
            Err(e) if via == ScenarioVia::Secondary => {
                if !is_stop_bail(&e) {
                    warn!("[llm] 从模型 send_chat_exchange 失败，回退主模型: {e:#}");
                }
                self.remember(ScenarioVia::Primary);
                let mut primary_config = retry_config.expect("从链路径已预留克隆");
                primary_config.model = self.primary.model_name();
                self.primary
                    .send_chat_exchange(
                        retry_messages.expect("从链路径已预留克隆"),
                        tools,
                        primary_config,
                    )
                    .await
            }
            Err(e) => Err(e),
        }
    }

    async fn complete_with_conversation(
        &self,
        system: &str,
        semi_static: &str,
        summary: Option<&str>,
        turns: &[super::client::ConversationTurn],
        current_prompt: &str,
    ) -> Result<String> {
        // 防御性覆盖：默认实现丢弃对话历史（semi_static/summary/turns），
        // 路由层必须保留完整历史转发到目标链
        let (via, _) = self.pick();
        self.remember(via);
        let client = self.target(via);
        let system = system.to_string();
        let semi_static = semi_static.to_string();
        let summary = summary.map(|s| s.to_string());
        let turns = turns.to_vec();
        let current_prompt = current_prompt.to_string();
        match client
            .complete_with_conversation(
                &system,
                &semi_static,
                summary.as_deref(),
                &turns,
                &current_prompt,
            )
            .await
        {
            Ok(v) => Ok(v),
            Err(e) if via == ScenarioVia::Secondary => {
                if !is_stop_bail(&e) {
                    warn!("[llm] 从模型 complete_with_conversation 失败，回退主模型: {e:#}");
                }
                self.remember(ScenarioVia::Primary);
                self.primary
                    .complete_with_conversation(
                        &system,
                        &semi_static,
                        summary.as_deref(),
                        &turns,
                        &current_prompt,
                    )
                    .await
            }
            Err(e) => Err(e),
        }
    }

    async fn complete_with_tools(
        &self,
        system: &str,
        prompt: &str,
        tools: &[ToolDefinition],
        executor: &dyn ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let (via, _) = self.pick();
        self.remember(via);
        let client = self.target(via);
        let system = system.to_string();
        let prompt = prompt.to_string();
        let tools = tools.to_vec();
        match client
            .complete_with_tools(&system, &prompt, &tools, executor, max_rounds)
            .await
        {
            Ok(v) => Ok(v),
            Err(e) if via == ScenarioVia::Secondary => {
                if !is_stop_bail(&e) {
                    warn!("[llm] 从模型 complete_with_tools 失败，回退主模型: {e:#}");
                }
                self.remember(ScenarioVia::Primary);
                self.primary
                    .complete_with_tools(&system, &prompt, &tools, executor, max_rounds)
                    .await
            }
            Err(e) => Err(e),
        }
    }

    async fn complete_with_conversation_and_tools(
        &self,
        system: &str,
        input: ConversationInput<'_>,
        tools: &[ToolDefinition],
        executor: &dyn ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let (via, _) = self.pick();
        self.remember(via);
        let client = self.target(via);
        let system = system.to_string();
        let semi_static = input.semi_static.to_string();
        let summary = input.summary.map(|s| s.to_string());
        let turns = input.turns.to_vec();
        let current_prompt = input.current_prompt.to_string();
        let tools = tools.to_vec();
        let retry_input = || ConversationInput {
            semi_static: &semi_static,
            summary: summary.as_deref(),
            turns: &turns,
            current_prompt: &current_prompt,
        };
        match client
            .complete_with_conversation_and_tools(
                &system,
                retry_input(),
                &tools,
                executor,
                max_rounds,
            )
            .await
        {
            Ok(v) => Ok(v),
            Err(e) if via == ScenarioVia::Secondary => {
                if !is_stop_bail(&e) {
                    warn!(
                        "[llm] 从模型 complete_with_conversation_and_tools 失败，回退主模型: {e:#}"
                    );
                }
                self.remember(ScenarioVia::Primary);
                self.primary
                    .complete_with_conversation_and_tools(
                        &system,
                        retry_input(),
                        &tools,
                        executor,
                        max_rounds,
                    )
                    .await
            }
            Err(e) => Err(e),
        }
    }

    // ── 身份与状态：按主链报告（容器身份即主模型）；调用后状态按最近使用端 ──

    fn provider_name(&self) -> String {
        self.primary.provider_name()
    }

    fn provider_info(&self) -> (super::direct_client::LlmProvider, String) {
        self.primary.provider_info()
    }

    fn context_window_tokens(&self) -> u32 {
        // 按主链报告：ConversationHistory 等上下文管理消费方以主决策链为准
        self.primary.context_window_tokens()
    }

    fn model_name(&self) -> String {
        self.primary.model_name()
    }

    fn supports_tool_calling(&self) -> bool {
        // tool loop 主路径在主链；从链若不支持工具而场景被路由过去，
        // 失败会走回退主链。此处按主链能力报告，避免场景未知时的误判。
        self.primary.supports_tool_calling()
    }

    fn force_rotate_model(&self) -> bool {
        self.primary.force_rotate_model()
    }

    fn take_last_reasoning_content(&self) -> Option<String> {
        self.state_client().take_last_reasoning_content()
    }

    fn take_last_tool_call_log(&self) -> Option<Vec<cyber_jianghu_protocol::EarthToolCall>> {
        self.state_client().take_last_tool_call_log()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::llm::openai_types::ChatExchangeConfig;
    use crate::component::llm::scenario::{DAILY_SUMMARY, THINK};
    use std::sync::atomic::{AtomicU8, Ordering};

    /// 记账 mock：model_name 标识主/从；可选注入失败
    struct MarkMock {
        name: &'static str,
        fail: bool,
        last_max_tokens: AtomicU8,
        last_model: std::sync::Mutex<String>,
    }

    #[async_trait]
    impl LlmClient for MarkMock {
        async fn complete(&self, _prompt: &str) -> Result<String> {
            if self.fail {
                anyhow::bail!("empty response body");
            }
            Ok(self.name.to_string())
        }
        async fn complete_with_system(&self, _system: &str, _prompt: &str) -> Result<String> {
            self.complete("").await
        }
        async fn send_chat_exchange(
            &self,
            _messages: Vec<ChatMessage>,
            _tools: Option<&[ToolDefinition]>,
            config: ChatExchangeConfig,
        ) -> Result<ChatExchangeResponse> {
            if let Some(t) = config.max_tokens {
                self.last_max_tokens
                    .store(t.min(255) as u8, Ordering::Relaxed);
            }
            if let Ok(mut m) = self.last_model.lock() {
                *m = config.model;
            }
            if self.fail {
                anyhow::bail!("empty response body");
            }
            Ok(ChatExchangeResponse {
                content: Some(self.name.to_string()),
                tool_calls: None,
                reasoning_content: None,
            })
        }
        fn model_name(&self) -> String {
            self.name.to_string()
        }
        fn provider_name(&self) -> String {
            "mock".to_string()
        }
    }

    fn routed(
        secondary: Option<Arc<MarkMock>>,
        routing: HashMap<String, ScenarioRouteConfig>,
    ) -> RoutedLlmClient {
        RoutedLlmClient::new(
            Arc::new(MarkMock {
                name: "Primary-Model",
                fail: false,
                last_max_tokens: AtomicU8::new(0),
                last_model: std::sync::Mutex::new(String::new()),
            }) as Arc<dyn LlmClient>,
            secondary.map(|s| s as Arc<dyn LlmClient>),
            routing,
        )
    }

    #[tokio::test]
    async fn default_light_scenario_routes_to_secondary() {
        // daily_summary 属内置轻量场景：未显式配置也走从
        let client = routed(
            Some(Arc::new(MarkMock {
                name: "Secondary-Model",
                fail: false,
                last_max_tokens: AtomicU8::new(0),
                last_model: std::sync::Mutex::new(String::new()),
            })),
            HashMap::new(),
        );
        let out = scenario::with_scenario(DAILY_SUMMARY, client.complete("hi")).await;
        assert_eq!(out.unwrap(), "Secondary-Model");
    }

    #[tokio::test]
    async fn main_decision_scenario_stays_primary() {
        // think 未显式配置：保持主链
        let client = routed(
            Some(Arc::new(MarkMock {
                name: "Secondary-Model",
                fail: false,
                last_max_tokens: AtomicU8::new(0),
                last_model: std::sync::Mutex::new(String::new()),
            })),
            HashMap::new(),
        );
        let out = scenario::with_scenario(THINK, client.complete("hi")).await;
        assert_eq!(out.unwrap(), "Primary-Model");
    }

    #[tokio::test]
    async fn explicit_config_overrides_default() {
        // think 显式配置走从 / 轻量场景显式配置回主，双向覆盖默认
        let mut routing = HashMap::new();
        routing.insert(
            THINK.0.to_string(),
            ScenarioRouteConfig {
                via: ScenarioVia::Secondary,
                max_tokens: None,
            },
        );
        routing.insert(
            DAILY_SUMMARY.0.to_string(),
            ScenarioRouteConfig {
                via: ScenarioVia::Primary,
                max_tokens: None,
            },
        );
        let client = routed(
            Some(Arc::new(MarkMock {
                name: "Secondary-Model",
                fail: false,
                last_max_tokens: AtomicU8::new(0),
                last_model: std::sync::Mutex::new(String::new()),
            })),
            routing,
        );
        let to_sec = scenario::with_scenario(THINK, client.complete("hi")).await;
        assert_eq!(to_sec.unwrap(), "Secondary-Model");
        let to_pri = scenario::with_scenario(DAILY_SUMMARY, client.complete("hi")).await;
        assert_eq!(to_pri.unwrap(), "Primary-Model");
    }

    #[tokio::test]
    async fn secondary_failure_falls_back_to_primary() {
        let client = routed(
            Some(Arc::new(MarkMock {
                name: "Secondary-Model",
                fail: true,
                last_max_tokens: AtomicU8::new(0),
                last_model: std::sync::Mutex::new(String::new()),
            })),
            HashMap::new(),
        );
        let out = scenario::with_scenario(DAILY_SUMMARY, client.complete("hi")).await;
        assert_eq!(out.unwrap(), "Primary-Model", "从模型失败应回退主模型");
    }

    #[tokio::test]
    async fn no_secondary_means_primary() {
        // 无从链（llm_secondary 未配置）：轻量场景也落主链
        let client = routed(None, HashMap::new());
        let out = scenario::with_scenario(DAILY_SUMMARY, client.complete("hi")).await;
        assert_eq!(out.unwrap(), "Primary-Model");
    }

    #[tokio::test]
    async fn chat_exchange_normalizes_model_and_applies_cap() {
        let secondary = Arc::new(MarkMock {
            name: "Secondary-Model",
            fail: false,
            last_max_tokens: AtomicU8::new(0),
            last_model: std::sync::Mutex::new(String::new()),
        });
        let mut routing = HashMap::new();
        routing.insert(
            DAILY_SUMMARY.0.to_string(),
            ScenarioRouteConfig {
                via: ScenarioVia::Secondary,
                max_tokens: Some(7),
            },
        );
        let client = routed(Some(secondary.clone()), routing);
        let config = ChatExchangeConfig {
            model: "Stale-Primary-Name".to_string(),
            temperature: 0.7,
            max_tokens: None,
            enable_thinking: None,
        };
        let resp = scenario::with_scenario(
            DAILY_SUMMARY,
            client.send_chat_exchange(Vec::new(), None, config),
        )
        .await
        .unwrap();
        assert_eq!(resp.content.unwrap(), "Secondary-Model");
        assert_eq!(
            secondary.last_max_tokens.load(Ordering::Relaxed),
            7,
            "场景 max_tokens 上限应下发到请求配置"
        );
    }

    #[tokio::test]
    async fn chat_exchange_secondary_failure_falls_back_with_normalized_model() {
        // 从链失败回退主链：模型名必须改写为主链名（陈旧模型名根治点），cap 保留
        let primary = Arc::new(MarkMock {
            name: "Primary-Model",
            fail: false,
            last_max_tokens: AtomicU8::new(0),
            last_model: std::sync::Mutex::new(String::new()),
        });
        let mut routing = HashMap::new();
        routing.insert(
            DAILY_SUMMARY.0.to_string(),
            ScenarioRouteConfig {
                via: ScenarioVia::Secondary,
                max_tokens: Some(9),
            },
        );
        let client = RoutedLlmClient::new(
            primary.clone(),
            Some(Arc::new(MarkMock {
                name: "Secondary-Model",
                fail: true,
                last_max_tokens: AtomicU8::new(0),
                last_model: std::sync::Mutex::new(String::new()),
            }) as Arc<dyn LlmClient>),
            routing,
        );
        let config = ChatExchangeConfig {
            model: "Stale-Name".to_string(),
            temperature: 0.7,
            max_tokens: None,
            enable_thinking: None,
        };
        let resp = scenario::with_scenario(
            DAILY_SUMMARY,
            client.send_chat_exchange(Vec::new(), None, config),
        )
        .await
        .unwrap();
        assert_eq!(resp.content.unwrap(), "Primary-Model");
        assert_eq!(
            primary.last_model.lock().unwrap().as_str(),
            "Primary-Model",
            "回退主链时请求模型应归一化为主链模型名"
        );
        assert_eq!(
            primary.last_max_tokens.load(Ordering::Relaxed),
            9,
            "cap 应保留"
        );
    }

    #[tokio::test]
    async fn primary_failure_does_not_double_retry() {
        // 主链失败：不触发任何二次重试（返回错误本身）
        let primary = Arc::new(CountingMock {
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let client =
            RoutedLlmClient::new(primary.clone() as Arc<dyn LlmClient>, None, HashMap::new());
        let _ = scenario::with_scenario(THINK, client.complete("hi")).await;
        assert_eq!(primary.calls.load(Ordering::Relaxed), 1, "主链失败不应重试");
    }

    /// 计数 mock：complete 恒失败并计数（实例内计数器）
    struct CountingMock {
        calls: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl LlmClient for CountingMock {
        async fn complete(&self, _prompt: &str) -> Result<String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            anyhow::bail!("primary hard failure")
        }
        async fn complete_with_system(&self, _system: &str, _prompt: &str) -> Result<String> {
            self.complete("").await
        }
    }

    #[tokio::test]
    async fn secondary_disabled_pauses_routing_to_primary() {
        // 从链独立停用：轻量场景临时回主（nextest 进程隔离下改全局标志无竞态）
        super::super::direct_client::set_secondary_disabled(true);
        let client = routed(
            Some(Arc::new(MarkMock {
                name: "Secondary-Model",
                fail: false,
                last_max_tokens: AtomicU8::new(0),
                last_model: std::sync::Mutex::new(String::new()),
            })),
            HashMap::new(),
        );
        let out = scenario::with_scenario(DAILY_SUMMARY, client.complete("hi")).await;
        assert_eq!(out.unwrap(), "Primary-Model", "从链停用时轻量场景应回主链");
        super::super::direct_client::set_secondary_disabled(false);
    }
}
