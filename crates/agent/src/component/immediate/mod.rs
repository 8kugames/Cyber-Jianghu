// ============================================================================
// 即时事件处理模块
// ============================================================================
//
// 处理 Server 下发的 ImmediateEvent（speak/whisper 等）
//
// 设计原则：
// - 即时意图通过普通 Intent 发送（使用当前 tick_id）
// - Server 允许即时动作在当前 tick 重复提交（覆盖之前的 intent）
// - Agent 自主决定：立即回应 / 延迟回应 / 不理会
//
// 决策架构：规则门控 + 轻量级 LLM
//   规则门控（<1ms）→ Ignore / Defer / MaybeRespond
//   MaybeRespond → 轻量级 LLM（4s 超时）→ RespondNow / DeferToMainTick
//
// 消息流：
//   Server -> ImmediateEvent -> ImmediateEventHandler -> 决策
//                                                     -> RespondNow: 发送 Intent
//                                                     -> lifecycle 主循环消费 immediate_event_buffer 存入工作记忆
// ============================================================================

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use cyber_jianghu_protocol::{
    AvailableAction, ClientMessage, ImmediateDecisionRules, ServerMessage, WorldEventType,
};

use crate::component::llm::LlmClientExt;
use crate::runtime::claw::LlmClientContainer;
use crate::soul::reflector::PersonaInfo;

// ============================================================================
// 常量
// ============================================================================

/// 即时意图优先级（高于普通意图）
pub const IMMEDIATE_INTENT_PRIORITY: i32 = 10;

// Type alias for rule validator callback
pub type RuleValidatorFn = dyn Fn(&str) -> std::result::Result<(), String> + Send + Sync;

// ============================================================================
// 即时事件
// ============================================================================

/// 待处理的即时事件
#[derive(Debug, Clone)]
pub struct PendingImmediateEvent {
    /// 事件唯一 ID
    pub event_id: Uuid,
    /// 事件类型
    pub event_type: WorldEventType,
    /// 事件描述
    pub description: String,
    /// 事件元数据
    pub metadata: serde_json::Value,
    /// 来源 Agent ID
    pub from_agent_id: Option<Uuid>,
    /// 接收时间
    pub received_at: Instant,
    /// 是否已决定响应
    pub responded: bool,
    /// 响应决策
    pub response_decision: Option<ResponseDecision>,
}

/// 即时响应决策
#[derive(Debug, Clone)]
pub enum ResponseDecision {
    /// 立即回应（发送 Intent）
    RespondNow {
        action_type: String,
        content: String,
        thought: String,
    },
    /// 延迟到主 tick 回应
    DeferToMainTick { reason: String },
    /// 不理会
    Ignore { reason: String },
}

// ============================================================================
// 即时决策器 Trait
// ============================================================================

/// 即时事件决策器（异步，支持 LLM 调用）
#[async_trait]
pub trait ImmediateDecisionMaker: Send + Sync {
    /// 决定如何响应即时事件
    async fn decide_response(
        &self,
        event: &PendingImmediateEvent,
        current_intent: Option<&str>,
        available_actions: &[AvailableAction],
    ) -> Option<ResponseDecision>;
}

// ============================================================================
// 即时事件处理器
// ============================================================================

/// 即时事件处理器
pub struct ImmediateEventHandler {
    /// 待处理事件队列
    pending_events: Arc<RwLock<Vec<PendingImmediateEvent>>>,

    /// 即时决策器
    decision_maker: Arc<dyn ImmediateDecisionMaker>,

    /// 即时意图发送通道（Handler -> 转发任务 -> WebSocket）
    intent_tx: Arc<RwLock<mpsc::Sender<ClientMessage>>>,

    /// 规则验证回调（Layer 1 + Layer 2，不涉及 LLM）
    rule_validator: Arc<RwLock<Option<Arc<RuleValidatorFn>>>>,

    /// 决策规则（数据驱动：TTL、队列容量等）
    rules: Arc<RwLock<ImmediateDecisionRules>>,

    /// 当前 tick_id（用于发送 Intent）
    current_tick_id: Arc<RwLock<i64>>,

    /// 当前正在执行的意图类型（用于冲突检测）
    current_intent_type: Arc<RwLock<Option<String>>>,

    /// HTTP API 状态（用于访问 SoulRecorder 记录即时意图）
    http_api_state: Arc<RwLock<Option<Arc<crate::infra::api::HttpApiState>>>>,
}

impl ImmediateEventHandler {
    /// 创建新的处理器
    pub fn new(
        decision_maker: Arc<dyn ImmediateDecisionMaker>,
        intent_tx: mpsc::Sender<ClientMessage>,
        rules: ImmediateDecisionRules,
    ) -> Self {
        Self {
            pending_events: Arc::new(RwLock::new(Vec::new())),
            decision_maker,
            intent_tx: Arc::new(RwLock::new(intent_tx)),
            rule_validator: Arc::new(RwLock::new(None)),
            rules: Arc::new(RwLock::new(rules)),
            current_tick_id: Arc::new(RwLock::new(0)),
            current_intent_type: Arc::new(RwLock::new(None)),
            http_api_state: Arc::new(RwLock::new(None)),
        }
    }

    /// 注入规则验证回调（地魂 Layer 1 + Layer 2）
    pub async fn set_rule_validator(&self, validator: Arc<RuleValidatorFn>) {
        let mut guard = self.rule_validator.write().await;
        *guard = Some(validator);
    }

    /// 更新决策规则（game_rules 热更新时调用）
    pub async fn update_rules(&self, new_rules: ImmediateDecisionRules) {
        let mut guard = self.rules.write().await;
        *guard = new_rules;
    }

    /// 更新当前 tick_id
    pub async fn set_tick_id(&self, tick_id: i64) {
        let mut guard = self.current_tick_id.write().await;
        *guard = tick_id;
    }

    /// 替换意图发送通道（连接建立后，绑定到 WebSocket 的 immediate_msg_tx）
    pub async fn replace_intent_channel(&self, new_tx: mpsc::Sender<ClientMessage>) {
        let mut guard = self.intent_tx.write().await;
        *guard = new_tx;
        info!("即时意图通道已绑定到 WebSocket");
    }

    /// 更新决策器配置（返回新的 Handler 实例）
    ///
    /// 由于 decision_maker 是不可变的 Arc，此方法创建新的 Handler 实例
    pub fn with_updated_decision_maker(&self, new_maker: Arc<dyn ImmediateDecisionMaker>) -> Self {
        Self {
            pending_events: self.pending_events.clone(),
            decision_maker: new_maker,
            intent_tx: self.intent_tx.clone(),
            rule_validator: self.rule_validator.clone(),
            rules: self.rules.clone(),
            current_tick_id: self.current_tick_id.clone(),
            current_intent_type: self.current_intent_type.clone(),
            http_api_state: self.http_api_state.clone(),
        }
    }

    /// 设置 HTTP API 状态（用于访问 SoulRecorder）
    pub async fn set_http_api_state(&self, state: Arc<crate::infra::api::HttpApiState>) {
        let mut guard = self.http_api_state.write().await;
        *guard = Some(state);
    }

    async fn get_soul_recorder(
        &self,
    ) -> Option<Arc<crate::infra::api::soul_cycle_recorder::SoulCycleRecorder>> {
        let api_state = {
            let guard = self.http_api_state.read().await;
            guard.as_ref()?.clone()
        };
        let agent_id = *api_state.agent_id.read().await;
        api_state.soul_recorder_for(agent_id).await
    }

    /// 设置当前正在执行的意图（用于冲突检测）
    pub async fn set_current_intent(&self, intent_type: Option<String>) {
        let mut guard = self.current_intent_type.write().await;
        *guard = intent_type;
    }

    /// 处理 Server 消息（提取 ImmediateEvent）
    pub async fn handle_server_message(&self, msg: ServerMessage) {
        if let ServerMessage::ImmediateEvent {
            event_id,
            event,
            deadline_ms: _,
        } = msg
        {
            let pending = PendingImmediateEvent {
                event_id,
                event_type: event.event_type,
                description: event.description.clone(),
                metadata: event.metadata.clone(),
                from_agent_id: event
                    .metadata
                    .get("from_agent_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok()),
                received_at: Instant::now(),
                responded: false,
                response_decision: None,
            };

            let max_pending = self.rules.read().await.max_pending_events;
            let mut queue = self.pending_events.write().await;
            if queue.len() >= max_pending {
                warn!(
                    "Pending events queue full ({}), dropping oldest",
                    queue.len()
                );
                queue.remove(0);
            }
            queue.push(pending);

            debug!(
                "Queued ImmediateEvent: id={}, type={}",
                event_id, event.event_type
            );
        } // 释放 pending_events 写锁后再决策，避免 RwLock 死锁

        // 触发即时决策（锁已释放，process_immediate_decision 会重新获取）
        self.process_immediate_decision().await;
    }

    /// 处理即时决策
    async fn process_immediate_decision(&self) {
        let event_ttl_ms = self.rules.read().await.event_ttl_ms;

        let events = {
            let mut queue = self.pending_events.write().await;
            let now = Instant::now();
            // 只处理未响应且未超时的
            let unresponded: Vec<_> = queue
                .iter_mut()
                .filter(|e| !e.responded)
                .filter(|e| now.duration_since(e.received_at).as_millis() as u64 <= event_ttl_ms)
                .map(|e| e.clone())
                .collect();
            unresponded
        };

        for mut event in events {
            let current_intent = {
                let guard = self.current_intent_type.read().await;
                guard.clone()
            };

            // 异步决策（支持 LLM 调用）
            let decision = self
                .decision_maker
                .decide_response(&event, current_intent.as_deref(), &[])
                .await;

            match decision {
                Some(ResponseDecision::RespondNow {
                    action_type,
                    content,
                    thought,
                }) => {
                    {
                        let validator_guard = self.rule_validator.read().await;
                        if let Some(ref validator) = *validator_guard
                            && let Err(reason) = validator(&action_type)
                        {
                            warn!(
                                "RespondNow rejected by rule validation: {} (event {})",
                                reason, event.event_id
                            );
                            event.responded = true;
                            continue;
                        }
                    }

                    let tick_id = *self.current_tick_id.read().await;
                    let response_uuid = uuid::Uuid::new_v4();
                    let intent = ClientMessage::Intent {
                        intent_id: Some(response_uuid),
                        tick_id,
                        agent_id: None,
                        thought_log: Some(thought.clone()),
                        action_type: action_type.clone(),
                        action_data: Some(serde_json::json!({
                            "content": content
                        })),
                        priority: IMMEDIATE_INTENT_PRIORITY,
                    };

                    if let Err(e) = self.intent_tx.read().await.send(intent).await {
                        error!("Failed to send immediate response Intent: {}", e);
                        if let Some(recorder) = self.get_soul_recorder().await {
                            let _ = recorder
                                .record_immediate(
                                    tick_id,
                                    &response_uuid.to_string(),
                                    None,
                                    "immediate_response",
                                    &action_type,
                                    Some(&serde_json::json!({"content": &content}).to_string()),
                                    Some(&content),
                                    "failed",
                                    Some(&e.to_string()),
                                )
                                .await;
                        }
                    } else {
                        info!(
                            "Sent immediate response for event {}: action={}, content='{}'",
                            event.event_id, action_type, content
                        );
                        event.responded = true;
                        if let Some(recorder) = self.get_soul_recorder().await {
                            let _ = recorder
                                .record_immediate(
                                    tick_id,
                                    &response_uuid.to_string(),
                                    None,
                                    "immediate_response",
                                    &action_type,
                                    Some(&serde_json::json!({"content": &content}).to_string()),
                                    Some(&content),
                                    "sent",
                                    None,
                                )
                                .await;
                        }
                    }
                }
                Some(ResponseDecision::DeferToMainTick { reason }) => {
                    debug!(
                        "Deferred ImmediateEvent {} to main tick: {}",
                        event.event_id, reason
                    );
                    event.response_decision = Some(ResponseDecision::DeferToMainTick { reason });
                }
                Some(ResponseDecision::Ignore { reason }) => {
                    debug!("Ignored ImmediateEvent {}: {}", event.event_id, reason);
                    event.responded = true;
                    event.response_decision = Some(ResponseDecision::Ignore { reason });
                }
                None => {}
            }

            // 更新事件状态
            {
                let mut queue = self.pending_events.write().await;
                if let Some(e) = queue.iter_mut().find(|q| q.event_id == event.event_id) {
                    *e = event;
                }
            }
        }
    }

    /// 获取延迟到主 tick 的事件（供主循环使用）
    pub async fn get_deferred_events(&self) -> Vec<PendingImmediateEvent> {
        let queue = self.pending_events.read().await;
        queue
            .iter()
            .filter(|e| {
                !e.responded
                    && matches!(
                        e.response_decision,
                        Some(ResponseDecision::DeferToMainTick { .. })
                    )
            })
            .cloned()
            .collect()
    }

    /// 清理已处理事件
    pub async fn cleanup_processed(&self) {
        let max_pending = self.rules.read().await.max_pending_events;
        let mut queue = self.pending_events.write().await;
        // 保留未响应和延迟的事件
        queue.retain(|e| {
            !e.responded
                || matches!(
                    e.response_decision,
                    Some(ResponseDecision::DeferToMainTick { .. })
                )
        });

        // 限制队列大小
        while queue.len() > max_pending {
            queue.remove(0);
        }
    }

    /// 停止处理器（清理待处理事件）
    pub async fn stop(&self) {
        let mut queue = self.pending_events.write().await;
        queue.clear();
    }
}

// ============================================================================
// 认知即时决策器（规则门控 + 轻量级 LLM）
// ============================================================================

/// LLM 即时决策的 JSON 输出格式
#[derive(Debug, Clone, Deserialize)]
struct ImmediateLlmResponse {
    /// 是否应该回应
    respond: bool,
    /// 回应的动作类型（speak / whisper）
    action_type: Option<String>,
    /// 回应内容
    content: Option<String>,
    /// 内心想法
    thought: Option<String>,
}

/// 认知即时决策器
///
/// 混合决策：规则门控（<1ms）过滤 90% 无关事件，
/// 仅对可能需要回应的事件调用轻量级 LLM（4s 超时）。
pub struct CognitiveImmediateDecisionMaker {
    /// LLM 客户端容器（共享，支持热重载）
    llm_container: LlmClientContainer,
    /// 角色人设摘要
    persona: PersonaInfo,
    /// 角色名称
    agent_name: String,
    /// 决策规则
    rules: ImmediateDecisionRules,
}

impl CognitiveImmediateDecisionMaker {
    pub fn new(
        llm_container: LlmClientContainer,
        persona: PersonaInfo,
        agent_name: String,
        rules: ImmediateDecisionRules,
    ) -> Self {
        Self {
            llm_container,
            persona,
            agent_name,
            rules,
        }
    }

    /// 规则门控（无 LLM，<1ms）
    ///
    /// 返回：
    /// - Some(decision) → 确定性决策（Ignore/Defer），无需 LLM
    /// - None → 需要调用 LLM 进一步判断
    fn rule_gate(
        &self,
        event: &PendingImmediateEvent,
        current_intent: Option<&str>,
    ) -> Option<ResponseDecision> {
        let content = event
            .metadata
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // 冲突检测：执行特定动作时不立即回应
        if let Some(intent_type) = current_intent {
            let has_conflict = self
                .rules
                .conflict_actions
                .iter()
                .any(|a| intent_type.contains(a));
            if has_conflict {
                return Some(ResponseDecision::DeferToMainTick {
                    reason: format!("当前正在执行 {}，延迟处理", intent_type),
                });
            }
        }

        // 空内容不回应
        if content.is_empty() {
            return Some(ResponseDecision::Ignore {
                reason: "事件内容为空".to_string(),
            });
        }

        // 非公开消息类型不立即回应（由主 tick 处理）
        if !matches!(event.event_type, WorldEventType::PublicMessage) {
            return Some(ResponseDecision::DeferToMainTick {
                reason: format!("非公开消息类型 {:?}，延迟到主 tick", event.event_type),
            });
        }

        // 公开消息 → 需要调用 LLM 判断是否回应
        None
    }

    /// 构建轻量级 LLM prompt
    fn build_prompt(&self, event: &PendingImmediateEvent) -> String {
        let content = event
            .metadata
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let sender = event
            .metadata
            .get("from_agent_name")
            .and_then(|v| v.as_str())
            .unwrap_or("某人");

        // 截断长事件
        let truncated = if content.len() > self.rules.max_event_context_chars {
            &content[..self.rules.max_event_context_chars]
        } else {
            content
        };

        let personality = self.personality_str();

        format!(
            r#"你是{name}，{personality}。
{sender}在你附近说：「{truncated}」

你需要快速判断：
1. 这句话是否与你有关或需要你回应？
2. 如果需要，你想说什么？

返回 JSON：
{{"respond": bool, "action_type": "speak", "content": "回应内容", "thought": "内心想法"}}

如果与你无关或不需要回应，respond 设为 false。
action_type 只能是 "speak" 或 "whisper"。
保持简短，1-2句话。"#,
            name = self.agent_name,
            personality = personality,
            sender = sender,
            truncated = truncated,
        )
    }

    fn personality_str(&self) -> String {
        let mut parts = Vec::new();
        if !self.persona.personality.is_empty() {
            parts.push(self.persona.personality.join("、"));
        }
        if !self.persona.values.is_empty() {
            parts.push(format!("信奉{}", self.persona.values.join("、")));
        }
        if parts.is_empty() {
            "江湖中人".to_string()
        } else {
            parts.join("，")
        }
    }

    /// 返回 LLM 调用超时（ms）
    ///
    /// 取 min(cognitive_timeout_ms, event_ttl_ms - 500)
    /// 确保在事件过期前留有安全余量
    fn effective_timeout_ms(&self) -> u64 {
        self.rules
            .cognitive_timeout_ms
            .min(self.rules.event_ttl_ms.saturating_sub(500))
    }
}

#[async_trait]
impl ImmediateDecisionMaker for CognitiveImmediateDecisionMaker {
    async fn decide_response(
        &self,
        event: &PendingImmediateEvent,
        current_intent: Option<&str>,
        _available_actions: &[AvailableAction],
    ) -> Option<ResponseDecision> {
        // 第一层：规则门控
        if let Some(decision) = self.rule_gate(event, current_intent) {
            return Some(decision);
        }

        // 第二层：轻量级 LLM 调用
        let prompt = self.build_prompt(event);
        let timeout_ms = self.effective_timeout_ms();

        let llm_result = {
            let llm = self.llm_container.read().await;
            let llm_ref = llm.clone();
            // 释放锁后调用 LLM
            drop(llm);
            tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                llm_ref.complete_json_with_system::<ImmediateLlmResponse>(
                    "你是一个即时回应决策器，根据角色人设和对话内容快速决定是否回应。只返回 JSON。",
                    &prompt,
                ),
            )
            .await
        };

        match llm_result {
            Ok(Ok(response)) => {
                if response.respond {
                    let action_type = response.action_type.unwrap_or_else(|| "speak".to_string());
                    // 验证 action_type 合法性
                    if action_type != "speak" && action_type != "whisper" {
                        warn!("LLM 返回非法 action_type '{}'，降级为 speak", action_type);
                    }
                    let valid_action = if action_type == "speak" || action_type == "whisper" {
                        action_type
                    } else {
                        "speak".to_string()
                    };
                    Some(ResponseDecision::RespondNow {
                        action_type: valid_action,
                        content: response.content.unwrap_or_else(|| "...".to_string()),
                        thought: response.thought.unwrap_or_else(|| "决定回应".to_string()),
                    })
                } else {
                    Some(ResponseDecision::Ignore {
                        reason: "LLM 判断无需回应".to_string(),
                    })
                }
            }
            Ok(Err(e)) => {
                // LLM 调用失败 → DeferToMainTick（fail-open）
                warn!("Immediate LLM call failed: {}，延迟到主 tick", e);
                Some(ResponseDecision::DeferToMainTick {
                    reason: format!("LLM 调用失败: {}", e),
                })
            }
            Err(_) => {
                // 超时 → DeferToMainTick
                debug!(
                    "Immediate LLM call timed out ({}ms)，延迟到主 tick",
                    timeout_ms
                );
                Some(ResponseDecision::DeferToMainTick {
                    reason: format!("LLM 调用超时 ({}ms)", timeout_ms),
                })
            }
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_event(content: &str) -> PendingImmediateEvent {
        PendingImmediateEvent {
            event_id: Uuid::new_v4(),
            event_type: WorldEventType::PublicMessage,
            description: format!("有人说: {}", content),
            metadata: serde_json::json!({
                "from_agent_id": Uuid::new_v4().to_string(),
                "content": content,
            }),
            from_agent_id: Some(Uuid::new_v4()),
            received_at: Instant::now(),
            responded: false,
            response_decision: None,
        }
    }

    #[tokio::test]
    async fn test_rule_gate_conflict_detection() {
        let persona = PersonaInfo::default();
        let rules = ImmediateDecisionRules {
            conflict_actions: vec!["move".into(), "fight".into()],
            ..ImmediateDecisionRules::default()
        };
        let llm: LlmClientContainer = Arc::new(tokio::sync::RwLock::new(Arc::new(
            crate::component::llm::MockLlmClient::with_response("{}"),
        )
            as Arc<dyn crate::component::llm::LlmClient>));
        let maker =
            CognitiveImmediateDecisionMaker::new(llm, persona, "测试角色".to_string(), rules);

        let event = create_test_event("喂！");
        let decision = maker.rule_gate(&event, Some("move"));
        assert!(
            matches!(decision, Some(ResponseDecision::DeferToMainTick { .. })),
            "conflict action should defer"
        );
    }

    #[tokio::test]
    async fn test_rule_gate_empty_content() {
        let persona = PersonaInfo::default();
        let rules = ImmediateDecisionRules::default();
        let llm: LlmClientContainer = Arc::new(tokio::sync::RwLock::new(Arc::new(
            crate::component::llm::MockLlmClient::with_response("{}"),
        )
            as Arc<dyn crate::component::llm::LlmClient>));
        let maker =
            CognitiveImmediateDecisionMaker::new(llm, persona, "测试角色".to_string(), rules);

        let event = create_test_event("");
        let decision = maker.rule_gate(&event, None);
        assert!(
            matches!(decision, Some(ResponseDecision::Ignore { .. })),
            "empty content should be ignored"
        );
    }

    #[tokio::test]
    async fn test_rule_gate_public_message_needs_llm() {
        let persona = PersonaInfo::default();
        let rules = ImmediateDecisionRules::default();
        let llm: LlmClientContainer = Arc::new(tokio::sync::RwLock::new(Arc::new(
            crate::component::llm::MockLlmClient::with_response("{}"),
        )
            as Arc<dyn crate::component::llm::LlmClient>));
        let maker =
            CognitiveImmediateDecisionMaker::new(llm, persona, "测试角色".to_string(), rules);

        let event = create_test_event("你好啊！");
        let decision = maker.rule_gate(&event, None);
        assert!(
            decision.is_none(),
            "public message with content should need LLM"
        );
    }

    #[tokio::test]
    async fn test_effective_timeout_calculation() {
        let rules = ImmediateDecisionRules {
            cognitive_timeout_ms: 4000,
            event_ttl_ms: 5000,
            ..ImmediateDecisionRules::default()
        };
        let llm: LlmClientContainer = Arc::new(tokio::sync::RwLock::new(Arc::new(
            crate::component::llm::MockLlmClient::with_response("{}"),
        )
            as Arc<dyn crate::component::llm::LlmClient>));
        let maker = CognitiveImmediateDecisionMaker::new(
            llm,
            PersonaInfo::default(),
            "测试角色".to_string(),
            rules,
        );

        // min(4000, 5000-500) = min(4000, 4500) = 4000
        assert_eq!(maker.effective_timeout_ms(), 4000);

        // Edge case: cognitive_timeout > event_ttl
        let rules_edge = ImmediateDecisionRules {
            cognitive_timeout_ms: 6000,
            event_ttl_ms: 3000,
            ..ImmediateDecisionRules::default()
        };
        let llm2: LlmClientContainer = Arc::new(tokio::sync::RwLock::new(Arc::new(
            crate::component::llm::MockLlmClient::with_response("{}"),
        )
            as Arc<dyn crate::component::llm::LlmClient>));
        let maker2 = CognitiveImmediateDecisionMaker::new(
            llm2,
            PersonaInfo::default(),
            "测试角色".to_string(),
            rules_edge,
        );
        // min(6000, 3000-500) = min(6000, 2500) = 2500
        assert_eq!(maker2.effective_timeout_ms(), 2500);
    }
}
