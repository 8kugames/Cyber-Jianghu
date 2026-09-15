// ============================================================================
// Fallback LLM 客户端（403/超时自动降级）+ 错误分类 + 共享熔断器
// ============================================================================
//
// FallbackLlmClient 策略：
// - 按序尝试所有模型（主模型 → fallback_models）
// - 成功后 sticky 到该模型（避免反复切换）
// - 仅对可恢复错误（403/429/超时）触发 fallback
//
// classify_llm_error / ErrorAction 是全 agent 统一的 LLM 错误分类入口
// （runtime/decision.rs 的认知重试也依赖它）。

use super::{ConversationInput, ConversationTurn, LlmClient};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

// ============================================================================

/// 禁用恢复冷却（按原因区分）。
/// - `rate_limit`（429 TPM/RPM）：按分钟窗口限流，60s 足够窗口翻转；
///   长冷却会让单模型 agent 在一次瞬时 429 后长时间无法工作。
/// - 其他原因（internal_server_error / empty_response 等）：维持保守长冷却。
const RATE_LIMIT_COOLDOWN_SECS: u64 = 60;
const MODEL_DISABLE_COOLDOWN_SECS: u64 = 3600;

/// 根据禁用原因返回冷却秒数
fn cooldown_secs_for_reason(reason: &str) -> u64 {
    if reason == "rate_limit" {
        RATE_LIMIT_COOLDOWN_SECS
    } else {
        MODEL_DISABLE_COOLDOWN_SECS
    }
}

// ============================================================================
// 共享 Circuit-Breaker
// ============================================================================
//
// 此前 `disabled_models` 仅存在于 `FallbackLlmClient`，
// 但 `run_tool_loop` 内部 `send_chat_exchange` 直接打到 `DirectLlmClient`，
// 完全绕过该表，导致 sensenova 抖动一次就被放大成 566 次 400。
//
// 抽 `SharedBreaker` 后，FallbackLlmClient 和 DirectLlmClient 共享同一份
// "已禁用 provider/model" 表，任意入口（fallback / tool_loop）都能命中。
// key = `"{provider}/{model}"`，多个 agent 共享同一 provider/model 时
// 也会一起退避（避免雪崩式打 sensenova）。

/// 共享 circuit-breaker 状态。
#[derive(Default)]
pub struct SharedBreaker {
    /// key: `"{provider}/{model}"`; value: (禁用开始时间, 冷却秒数)
    disabled: std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, u64)>>,
}

impl std::fmt::Debug for SharedBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let disabled = self.disabled.lock().expect("lock poisoned");
        f.debug_struct("SharedBreaker")
            .field("disabled_keys", &disabled.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SharedBreaker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 查询 key 是否被禁用。返回 `Some(remaining_secs)` 表示仍在冷却，
    /// `None` 表示可用（不在表内或已过期）。
    pub fn is_disabled(&self, key: &str) -> Option<u64> {
        let mut disabled = self.disabled.lock().expect("lock poisoned");
        // 清理过期项（按各条目自身冷却时长）
        let now = std::time::Instant::now();
        disabled.retain(|_, (ts, cooldown)| now.duration_since(*ts).as_secs() < *cooldown);
        disabled.get(key).map(|(ts, cooldown)| {
            let elapsed = now.duration_since(*ts).as_secs();
            cooldown.saturating_sub(elapsed)
        })
    }

    /// 标记 key 禁用（携带本次冷却时长）
    pub fn disable(&self, key: String, cooldown_secs: u64) {
        let mut disabled = self.disabled.lock().expect("lock poisoned");
        disabled.insert(key, (std::time::Instant::now(), cooldown_secs));
    }
}

// ============================================================================
// 统一错误分类 — 三处消费者共享同一份分类逻辑
// ============================================================================

/// LLM 调用错误的处理策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorAction {
    /// 放弃 — 错误不可恢复，无需重试（auth、context 超长等）
    GiveUp,
    /// 切换到下一个 model/provider（403 额度不足、404 模型不存在等）
    Fallback,
    /// 禁用当前 model 后切换到下一个（429 限流、provider 炸了等）
    FallbackAndDisable,
    /// 重试（网络瞬时故障、连接超时等）
    Retry,
}

/// 根据 error 内容分类 LLM 调用错误
///
/// 返回 `(ErrorAction, &'static str)`，第二元素为 disable_model 的简短原因。
///
/// 三处消费者：
/// - `call_with_fallback + call_streaming_with_fallback`:
///   `Fallback | FallbackAndDisable | Retry` → 继续轮询下一模型
///   `FallbackAndDisable` → disable_model + rotate
/// - `decision.rs` retry loop: `GiveUp | Fallback | FallbackAndDisable` → break
pub fn classify_llm_error(error: &anyhow::Error) -> (ErrorAction, &'static str) {
    let msg = format!("{:#}", error);

    // ── Permanent: 确定性的，重试/fallback 无意义 ──────────────
    if msg.contains("exceeds max context window")
        || msg.contains("Prompt too long")
        || msg.contains("context_length_exceeded")
        || msg.contains("maximum context length")
    {
        return (ErrorAction::GiveUp, "context_too_long");
    }

    // ── Config/Model: 换模型可能解决 ────────────────────────────
    if msg.contains("LLM API error 404")
        || msg.contains("LLM streaming API error 404")
        || msg.contains("AllocationQuota")
    {
        return (ErrorAction::Fallback, "model_not_found_or_quota");
    }

    // 403 也可能是配额问题
    if msg.contains("LLM API error 403") || msg.contains("LLM streaming API error 403") {
        return (ErrorAction::Fallback, "forbidden_or_quota");
    }

    // ── Rate limit: 禁用模型（rate_limit 60s 冷却，覆盖分钟窗口）────────────
    if msg.contains("429")
        || msg.contains("rate_limit")
        || msg.contains("Too Many Requests")
        || msg.contains("LLM API error 429")
        || msg.contains("LLM streaming API error 429")
    {
        return (ErrorAction::FallbackAndDisable, "rate_limit");
    }

    // ── Provider 内部错误: 如 sensenova 把 503 包装成 400 internal_server_error ──
    if msg.contains("internal_server_error") {
        return (ErrorAction::FallbackAndDisable, "internal_server_error");
    }

    // ── 空响应: 模型偶尔返回空 body / 空 content ────────────────
    if msg.contains("empty response body")
        || msg.contains("response content is empty")
        || msg.contains("returned empty response")
    {
        return (ErrorAction::FallbackAndDisable, "empty_response");
    }

    // ── 400 Bad Request: 模型能力不匹配（如 "only support stream mode"）─
    if msg.contains("LLM API error 400") || msg.contains("LLM streaming API error 400") {
        return (ErrorAction::Fallback, "bad_request");
    }

    // ── 连接/请求失败: 网络瞬时故障 ─────────────────────────────
    if msg.contains("Failed to send request to LLM API")
        || msg.contains("error sending request for url")
        || msg.contains("does not support http call")
    {
        return (ErrorAction::Retry, "connection_failed");
    }

    // 未知错误 — 保守起见，允许 fallback
    (ErrorAction::Retry, "unknown_error")
}

/// Fallback LLM 客户端
///
/// 主模型 403（额度耗尽）或超时时，自动切换到备用模型。
/// 所有模型共享同一 provider/api_key，仅 model name 不同。
///
/// 一旦某个 fallback 成功，后续调用优先使用该模型（sticky fallback）。
///
/// Idle 旋转机制：连续 idle 达到阈值时自动切换到下一个模型。
pub struct FallbackLlmClient {
    /// LLM 客户端列表（index 0 = 主模型，1.. = fallback）
    clients: Vec<Arc<dyn LlmClient>>,
    /// 当前活跃客户端索引
    active: Arc<std::sync::atomic::AtomicUsize>,
    /// 连续 idle 计数（每个模型独立计数）
    idle_counts: Arc<std::sync::Mutex<Vec<usize>>>,
    /// 旋转阈值
    idle_threshold: usize,
    /// 标记为不可用的模型索引 + (disable 时间戳, 冷却秒数)
    disabled_models:
        Arc<std::sync::Mutex<std::collections::HashMap<usize, (std::time::Instant, u64)>>>,
    /// 共享 circuit-breaker：写入时同步到下层 DirectLlmClient，
    /// 使 `run_tool_loop` 内部 `send_chat_exchange` 也能命中。
    shared_breaker: Arc<SharedBreaker>,
}

impl FallbackLlmClient {
    /// 创建 Fallback 客户端
    ///
    /// `clients` 不应为空，index 0 是主模型。
    pub fn new(clients: Vec<Arc<dyn LlmClient>>) -> Self {
        assert!(
            !clients.is_empty(),
            "FallbackLlmClient needs at least one client"
        );
        let count = clients.len();
        Self {
            clients,
            active: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            idle_counts: Arc::new(std::sync::Mutex::new(vec![0; count])),
            idle_threshold: 5, // 默认阈值
            disabled_models: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            shared_breaker: Arc::new(SharedBreaker::new()),
        }
    }

    /// 设置 idle 旋转阈值
    pub fn with_idle_threshold(mut self, threshold: usize) -> Self {
        self.idle_threshold = threshold;
        self
    }

    /// 注入共享 circuit-breaker（由 build_fallback_client 调用，
    /// 必须与下层 DirectLlmClient 持有的 Arc 指向同一实例）
    pub fn with_shared_breaker(mut self, breaker: Arc<SharedBreaker>) -> Self {
        self.shared_breaker = breaker;
        self
    }

    /// 强制切换到下一个模型
    ///
    /// 将 active 索引前进一位（环绕）。返回 true 表示切换成功，
    /// false 表示只有一个模型无法切换。
    pub fn force_rotate(&self) -> bool {
        if self.clients.len() <= 1 {
            return false;
        }
        let old = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let new = (old + 1) % self.clients.len();
        self.active.store(new, std::sync::atomic::Ordering::Relaxed);
        tracing::warn!("强制切换 LLM 模型: #{} → #{}", old, new);
        true
    }

    /// 记录当前模型的 idle 行为，自动切换到下一个模型
    ///
    /// 如果当前模型连续 idle 达到阈值，则标记为不可用并切换。
    /// 返回 true 表示发生了切换，false 表示未达到阈值。
    pub fn record_idle(&self) -> bool {
        let current_idx = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let mut idle_counts = self.idle_counts.lock().expect("lock poisoned");
        let mut disabled = self.disabled_models.lock().expect("lock poisoned");

        // 增加当前模型的 idle 计数
        idle_counts[current_idx] += 1;
        let count = idle_counts[current_idx];

        if count >= self.idle_threshold {
            // 标记当前模型为不可用（idle 非限流原因，走保守长冷却）
            disabled.insert(
                current_idx,
                (std::time::Instant::now(), MODEL_DISABLE_COOLDOWN_SECS),
            );
            tracing::warn!(
                "LLM 模型 #{} 连续 idle {} 次，达到阈值 {}，标记为不可用",
                current_idx,
                count,
                self.idle_threshold
            );

            // 切换到下一个可用模型
            drop(disabled);
            self.rotate_to_next_available();
            return true;
        }

        false
    }

    /// 切换到下一个可用模型
    ///
    /// 跳过已标记为不可用的模型。如果所有模型都不可用，则保持当前状态。
    fn rotate_to_next_available(&self) {
        let start = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let disabled = self.disabled_models.lock().expect("lock poisoned");

        for offset in 1..=self.clients.len() {
            let idx = (start + offset) % self.clients.len();
            if !disabled.contains_key(&idx) {
                let old = self.active.load(std::sync::atomic::Ordering::Relaxed);
                self.active.store(idx, std::sync::atomic::Ordering::Relaxed);
                tracing::warn!("LLM idle 旋转：模型 #{} → #{} (跳过不可用模型)", old, idx);
                return;
            }
        }

        tracing::error!("所有 LLM 模型都已标记为不可用，保持当前模型");
    }

    /// 标记指定模型为不可用（429 circuit breaker 等），冷却时长按原因区分
    fn disable_model(&self, idx: usize, reason: &str) {
        let cooldown_secs = cooldown_secs_for_reason(reason);
        let mut disabled = self.disabled_models.lock().expect("lock poisoned");
        if disabled
            .insert(idx, (std::time::Instant::now(), cooldown_secs))
            .is_none()
        {
            // 同步写入共享 breaker：key = "{provider}/{model}"，
            // 使下层 DirectLlmClient 在 tool_loop 内部 send_chat_exchange 时也能命中。
            let key = format!(
                "{}/{}",
                self.clients[idx].provider_name(),
                self.clients[idx].model_name()
            );
            self.shared_breaker.disable(key, cooldown_secs);

            tracing::warn!(
                "LLM 模型 #{} 标记为不可用 (原因: {}，冷却 {}s)，已禁用模型: {:?}",
                idx,
                reason,
                cooldown_secs,
                disabled.keys().collect::<Vec<_>>()
            );
            drop(disabled);
            self.rotate_to_next_available();
        }
    }

    fn reenable_expired(&self) {
        let mut disabled = self.disabled_models.lock().expect("lock poisoned");
        let now = std::time::Instant::now();
        let expired: Vec<usize> = disabled
            .iter()
            .filter(|&(_, (ts, cooldown))| now.duration_since(*ts).as_secs() >= *cooldown)
            .map(|(&idx, _)| idx)
            .collect();

        for idx in &expired {
            disabled.remove(idx);
        }
        drop(disabled);

        if !expired.is_empty() {
            tracing::info!(
                "429 circuit breaker 恢复: 模型 {:?} 已重新激活（冷却期已过）",
                expired
            );
        }
    }

    /// 重置当前模型的 idle 计数（当模型返回非 idle 结果时调用）
    pub fn reset_idle_count(&self) {
        let current_idx = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let mut idle_counts = self.idle_counts.lock().expect("lock poisoned");
        let old_count = idle_counts[current_idx];
        if old_count > 0 {
            idle_counts[current_idx] = 0;
            tracing::debug!("LLM 模型 #{} idle 计数重置: {} → 0", current_idx, old_count);
        }
    }

    /// 获取当前活跃客户端
    fn active_client(&self) -> Arc<dyn LlmClient> {
        let idx = self.active.load(std::sync::atomic::Ordering::Relaxed);
        self.clients[idx.min(self.clients.len() - 1)].clone()
    }

    /// 带自动 fallback 的调用核心（返回类型由闭包决定）。
    ///
    /// 策略：从 active index 开始，失败时尝试后续所有客户端。
    /// 一旦成功，sticky 到该客户端。`FallbackAndDisable` 会同步写 shared_breaker。
    /// `streaming=true` 时日志带 streaming 标注，并跳过 non-streaming 400 提示。
    async fn call_with_fallback_core<F, Fut, T>(&self, f: F, streaming: bool) -> Result<T>
    where
        F: Fn(Arc<dyn LlmClient>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        self.reenable_expired();
        let start = self.active.load(std::sync::atomic::Ordering::Relaxed);
        let mut last_err = None;
        let mode = if streaming { "streaming " } else { "" };

        for offset in 0..self.clients.len() {
            let idx = (start + offset) % self.clients.len();

            // 跳过已被 circuit breaker 禁用的模型（短锁，不跨 await）
            if self
                .disabled_models
                .lock()
                .expect("lock poisoned")
                .contains_key(&idx)
            {
                continue;
            }

            let client = self.clients[idx].clone();

            match f(client).await {
                Ok(value) => {
                    if offset > 0 {
                        tracing::warn!(
                            "LLM {}fallback 成功：切换到客户端 #{} (主用 #{}）",
                            mode,
                            idx,
                            start
                        );
                        // sticky：后续调用使用此客户端
                        self.active.store(idx, std::sync::atomic::Ordering::Relaxed);
                    }
                    return Ok(value);
                }
                Err(e) => {
                    let (action, reason) = classify_llm_error(&e);
                    let is_fallback = matches!(
                        action,
                        ErrorAction::Fallback
                            | ErrorAction::FallbackAndDisable
                            | ErrorAction::Retry
                    );
                    tracing::warn!(
                        "LLM {}客户端 #{} 调用失败 (action={:?}): {}",
                        mode,
                        idx,
                        action,
                        e
                    );
                    if action == ErrorAction::FallbackAndDisable {
                        self.disable_model(idx, reason);
                    }
                    if !streaming {
                        let err_msg = format!("{:#}", e);
                        if err_msg.contains("LLM API error 400")
                            && !err_msg.contains("Prompt too long")
                        {
                            tracing::warn!(
                                "提示: 模型可能不支持 non-streaming，建议在 agent.yaml 中设置 prefer_stream: true"
                            );
                        }
                    }
                    if !is_fallback {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        }

        // 所有客户端都失败
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("所有 LLM 客户端均失败")))
    }

    /// 执行带 fallback 的调用（返回类型由闭包决定）
    async fn call_with_fallback<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: Fn(Arc<dyn LlmClient>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        self.call_with_fallback_core(f, false).await
    }

    /// 流式调用的 fallback 逻辑
    ///
    /// 连接阶段失败（如 403/超时）自动切换到下一个 provider。
    /// 流中途失败直接返回 Err（无法中途切换）。
    /// 返回 (stream, provider_name, model_name)
    async fn call_streaming_with_fallback<F, Fut>(
        &self,
        f: F,
    ) -> Result<(super::super::streaming::LlmStream, String, String)>
    where
        F: Fn(Arc<dyn LlmClient>) -> Fut,
        Fut: std::future::Future<Output = Result<super::super::streaming::LlmStream>>,
    {
        // future 在 async 块外创建（f 仅被借用，包装闭包才能多次调用）
        self.call_with_fallback_core(
            move |client: Arc<dyn LlmClient>| {
                let fut = f(client.clone());
                async move {
                    let stream = fut.await?;
                    Ok((stream, client.provider_name(), client.model_name()))
                }
            },
            true,
        )
        .await
    }
}

#[allow(private_interfaces)]
#[async_trait]
impl LlmClient for FallbackLlmClient {
    fn force_rotate_model(&self) -> bool {
        self.force_rotate()
    }

    async fn complete(&self, prompt: &str) -> Result<String> {
        let prompt = prompt.to_string();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let prompt = prompt.clone();
            async move { client.complete(&prompt).await }
        })
        .await
    }

    async fn complete_with_system(&self, system: &str, prompt: &str) -> Result<String> {
        let system = system.to_string();
        let prompt = prompt.to_string();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let system = system.clone();
            let prompt = prompt.clone();
            async move { client.complete_with_system(&system, &prompt).await }
        })
        .await
    }

    fn supports_tool_calling(&self) -> bool {
        self.active_client().supports_tool_calling()
    }

    async fn send_chat_exchange(
        &self,
        messages: Vec<super::super::openai_types::ChatMessage>,
        tools: Option<&[super::super::tool_types::ToolDefinition]>,
        config: super::super::openai_types::ChatExchangeConfig,
    ) -> Result<super::super::openai_types::ChatExchangeResponse> {
        // 走 call_with_fallback：跳过已 disabled 的客户端，
        // FallbackAndDisable 时同步写 shared_breaker。
        // 此前直调 active_client 会在 tool_loop 内部绕过 circuit breaker。
        let tools_opt = tools.map(|t| t.to_vec());
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let messages = messages.clone();
            let tools_inner = tools_opt.clone();
            let config = config.clone();
            async move {
                client
                    .send_chat_exchange(messages, tools_inner.as_deref(), config)
                    .await
            }
        })
        .await
    }

    fn provider_name(&self) -> String {
        self.active_client().provider_name()
    }

    fn model_name(&self) -> String {
        self.active_client().model_name()
    }

    fn context_window_tokens(&self) -> u32 {
        self.active_client().context_window_tokens()
    }

    fn provider_info(&self) -> (super::super::direct_client::LlmProvider, String) {
        self.active_client().provider_info()
    }

    fn take_last_reasoning_content(&self) -> Option<String> {
        self.active_client().take_last_reasoning_content()
    }

    fn take_last_tool_call_log(&self) -> Option<Vec<cyber_jianghu_protocol::EarthToolCall>> {
        self.active_client().take_last_tool_call_log()
    }

    async fn complete_with_tools(
        &self,
        system: &str,
        prompt: &str,
        tools: &[super::super::tool_types::ToolDefinition],
        executor: &dyn super::super::tool_types::ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let system = system.to_string();
        let prompt = prompt.to_string();
        let tools = tools.to_vec();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let system = system.clone();
            let prompt = prompt.clone();
            let tools = tools.clone();
            async move {
                client
                    .complete_with_tools(&system, &prompt, &tools, executor, max_rounds)
                    .await
            }
        })
        .await
    }

    async fn complete_with_conversation_and_tools(
        &self,
        system: &str,
        input: ConversationInput<'_>,
        tools: &[super::super::tool_types::ToolDefinition],
        executor: &dyn super::super::tool_types::ToolExecutor,
        max_rounds: usize,
    ) -> Result<String> {
        let system = system.to_string();
        let semi_static = input.semi_static.to_string();
        let summary_owned = input.summary.map(|s| s.to_string());
        let turns = input.turns.to_vec();
        let current_prompt = input.current_prompt.to_string();
        let tools = tools.to_vec();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let system = system.clone();
            let semi_static = semi_static.clone();
            let summary = summary_owned.clone();
            let turns = turns.clone();
            let current_prompt = current_prompt.clone();
            let tools = tools.clone();
            async move {
                client
                    .complete_with_conversation_and_tools(
                        &system,
                        ConversationInput {
                            semi_static: &semi_static,
                            summary: summary.as_deref(),
                            turns: &turns,
                            current_prompt: &current_prompt,
                        },
                        &tools,
                        executor,
                        max_rounds,
                    )
                    .await
            }
        })
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
        let system = system.to_string();
        let semi_static = semi_static.to_string();
        let summary_owned = summary.map(|s| s.to_string());
        let turns = turns.to_vec();
        let current_prompt = current_prompt.to_string();
        self.call_with_fallback(move |client: Arc<dyn LlmClient>| {
            let system = system.clone();
            let semi_static = semi_static.clone();
            let summary = summary_owned.clone();
            let turns = turns.clone();
            let current_prompt = current_prompt.clone();
            async move {
                client
                    .complete_with_conversation(
                        &system,
                        &semi_static,
                        summary.as_deref(),
                        &turns,
                        &current_prompt,
                    )
                    .await
            }
        })
        .await
    }

    fn complete_streaming<'a>(
        &'a self,
        system: &'a str,
        prompt: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<super::super::streaming::LlmStream>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            use super::super::direct_client::LlmProvider;

            let system = system.to_string();
            let prompt = prompt.to_string();
            let prompt_chars = super::super::streaming::plain_prompt_chars(&system, &prompt);
            let system_hash = crate::soul::actor::compute_system_hash(&system);
            let (stream, provider_str, model) = self
                .call_streaming_with_fallback(move |client: Arc<dyn LlmClient>| {
                    let system = system.clone();
                    let prompt = prompt.clone();
                    async move { client.complete_streaming(&system, &prompt).await }
                })
                .await?;

            let provider = LlmProvider::parse(&provider_str).unwrap_or(LlmProvider::OpenClaw);
            Ok(super::super::streaming::wrap_usage_tracking(
                stream,
                provider,
                model,
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
        Box<
            dyn std::future::Future<Output = Result<super::super::streaming::LlmStream>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            use super::super::direct_client::LlmProvider;

            let system = system.to_string();
            let semi_static = semi_static.to_string();
            let summary_owned = summary.map(|s| s.to_string());
            let turns = turns.to_vec();
            let current_prompt = current_prompt.to_string();
            let system_hash = crate::soul::actor::compute_system_hash(&system);
            let prompt_chars = super::super::streaming::conversation_prompt_chars(
                &system,
                &semi_static,
                summary_owned.as_deref(),
                &turns,
                &current_prompt,
            );
            let (stream, provider_str, model) = self
                .call_streaming_with_fallback(move |client: Arc<dyn LlmClient>| {
                    let system = system.clone();
                    let semi_static = semi_static.clone();
                    let summary = summary_owned.clone();
                    let turns = turns.clone();
                    let current_prompt = current_prompt.clone();
                    async move {
                        client
                            .complete_conversation_streaming(
                                &system,
                                &semi_static,
                                summary.as_deref(),
                                &turns,
                                &current_prompt,
                            )
                            .await
                    }
                })
                .await?;

            let provider = LlmProvider::parse(&provider_str).unwrap_or(LlmProvider::OpenClaw);
            Ok(super::super::streaming::wrap_usage_tracking(
                stream,
                provider,
                model,
                system_hash,
                prompt_chars,
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cooldown_secs_for_reason() {
        // rate_limit：分钟窗口限流 → 短冷却
        assert_eq!(
            cooldown_secs_for_reason("rate_limit"),
            RATE_LIMIT_COOLDOWN_SECS
        );
        // 其他原因：维持保守长冷却
        assert_eq!(
            cooldown_secs_for_reason("internal_server_error"),
            MODEL_DISABLE_COOLDOWN_SECS
        );
        assert_eq!(
            cooldown_secs_for_reason("empty_response"),
            MODEL_DISABLE_COOLDOWN_SECS
        );
        assert_eq!(cooldown_secs_for_reason(""), MODEL_DISABLE_COOLDOWN_SECS);
    }
}
