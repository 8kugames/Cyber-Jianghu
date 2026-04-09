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
// 消息流：
//   Server -> ImmediateEvent -> ImmediateEventHandler -> 决策（RespondNow/Defer/Ignore）
//                                                    -> RespondNow: 发送 Intent (speak)
//                                                    -> lifecycle 主循环消费 immediate_event_buffer 存入工作记忆
// ============================================================================

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use cyber_jianghu_protocol::{
    AvailableAction, ClientMessage, ServerMessage, WorldEventType,
};

// ============================================================================
// 常量
// ============================================================================

/// 即时决策超时（毫秒）
const IMMEDIATE_DECISION_TIMEOUT_MS: u64 = 5000;

/// 最大待处理即时事件队列
const MAX_PENDING_EVENTS: usize = 32;



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
    /// 立即回应（发送普通 Intent）
    RespondNow {
        content: String,
        thought: String,
    },
    /// 延迟到主 tick 回应
    DeferToMainTick {
        reason: String,
    },
    /// 不理会
    Ignore {
        reason: String,
    },
}

// ============================================================================
// 即时决策器 Trait
// ============================================================================

/// 即时事件决策器
pub trait ImmediateDecisionMaker: Send + Sync {
    /// 决定如何响应即时事件
    fn decide_response(
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
    /// RwLock 允许运行时替换（连接后绑定到 WebSocket 的 immediate_msg_tx）
    intent_tx: Arc<RwLock<mpsc::Sender<ClientMessage>>>,

    /// 当前 tick_id（用于发送 Intent）
    current_tick_id: Arc<RwLock<i64>>,

    /// 当前正在执行的意图类型（用于冲突检测）
    current_intent_type: Arc<RwLock<Option<String>>>,

    /// 是否正在运行
    running: Arc<AtomicBool>,
}

impl ImmediateEventHandler {
    /// 创建新的处理器
    pub fn new(
        decision_maker: Arc<dyn ImmediateDecisionMaker>,
        intent_tx: mpsc::Sender<ClientMessage>,
    ) -> Self {
        Self {
            pending_events: Arc::new(RwLock::new(Vec::new())),
            decision_maker,
            intent_tx: Arc::new(RwLock::new(intent_tx)),
            current_tick_id: Arc::new(RwLock::new(0)),
            current_intent_type: Arc::new(RwLock::new(None)),
            running: Arc::new(AtomicBool::new(true)),
        }
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

            let mut queue = self.pending_events.write().await;
            if queue.len() >= MAX_PENDING_EVENTS {
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

            // 触发即时决策
            self.process_immediate_decision().await;
        }
    }

    /// 处理即时决策
    async fn process_immediate_decision(&self) {
        let events = {
            let mut queue = self.pending_events.write().await;
            let now = Instant::now();
            // 只处理未响应且未超时的
            let unresponded: Vec<_> = queue
                .iter_mut()
                .filter(|e| !e.responded)
                .filter(|e| now.duration_since(e.received_at).as_millis() as u64
                    <= IMMEDIATE_DECISION_TIMEOUT_MS)
                .map(|e| e.clone())
                .collect();
            unresponded
        };

        for mut event in events {
            let current_intent = {
                let guard = self.current_intent_type.read().await;
                guard.clone()
            };

            // 决策（使用空列表，由 LLM 或上层提供可用动作）
            let decision = self.decision_maker.decide_response(&event, current_intent.as_deref(), &[]);

            match decision {
                Some(ResponseDecision::RespondNow { content, thought }) => {
                    // 发送普通 Intent（使用当前 tick_id）
                    let tick_id = *self.current_tick_id.read().await;
                    let intent = ClientMessage::Intent {
                        intent_id: None,
                        tick_id,
                        agent_id: None,
                        thought_log: Some(thought),
                        action_type: "speak".to_string(),
                        action_data: Some(serde_json::json!({
                            "content": content
                        })),
                        priority: 10, // 即时回应高优先级
                    };

                    if let Err(e) = self.intent_tx.read().await.send(intent).await {
                        error!("Failed to send immediate response Intent: {}", e);
                    } else {
                        info!(
                            "Sent immediate response for event {}: '{}'",
                            event.event_id, content
                        );
                        event.responded = true;
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
                    debug!(
                        "Ignored ImmediateEvent {}: {}",
                        event.event_id, reason
                    );
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
        while queue.len() > MAX_PENDING_EVENTS {
            queue.remove(0);
        }
    }

    /// 停止处理器
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// 检查是否正在运行
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

// ============================================================================
// 默认即时决策器（基于规则）
// ============================================================================

/// 基于规则的即时决策器
pub struct RuleBasedImmediateDecisionMaker {
    /// 是否启用即时响应
    enable_immediate_response: bool,
}

impl RuleBasedImmediateDecisionMaker {
    pub fn new() -> Self {
        Self {
            enable_immediate_response: true,
        }
    }

    pub fn with_enable(enable: bool) -> Self {
        Self {
            enable_immediate_response: enable,
        }
    }
}

impl Default for RuleBasedImmediateDecisionMaker {
    fn default() -> Self {
        Self::new()
    }
}

impl ImmediateDecisionMaker for RuleBasedImmediateDecisionMaker {
    fn decide_response(
        &self,
        event: &PendingImmediateEvent,
        current_intent: Option<&str>,
        _available_actions: &[AvailableAction],
    ) -> Option<ResponseDecision> {
        if !self.enable_immediate_response {
            return Some(ResponseDecision::Ignore {
                reason: "即时响应已禁用".to_string(),
            });
        }

        // 从元数据提取内容
        let content = event
            .metadata
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // 冲突检测：移动中不立即回应
        if let Some(intent_type) = current_intent {
            let conflict_actions = ["move", "travel", "gather", "craft", "fight"];
            if conflict_actions.iter().any(|a| intent_type.contains(a)) {
                return Some(ResponseDecision::DeferToMainTick {
                    reason: format!("当前正在执行 {}，延迟处理", intent_type),
                });
            }
        }

        // 被直接呼唤：立即回应
        let is_being_called = content.contains("喂")
            || content.contains("哎")
            || content.contains("这位")
            || content.contains("侠客")
            || content.contains("朋友");

        if is_being_called && content.len() < 50 {
            return Some(ResponseDecision::RespondNow {
                content: "何事？".to_string(),
                thought: format!("被人呼唤 '{}'，立即回应", content),
            });
        }

        // 普通对话：延迟到主 tick
        if matches!(event.event_type, WorldEventType::PublicMessage) && !content.is_empty() {
            return Some(ResponseDecision::DeferToMainTick {
                reason: format!("普通对话 '{}'，延迟到主 tick", content),
            });
        }

        // 默认不响应
        Some(ResponseDecision::Ignore {
            reason: "未匹配响应规则".to_string(),
        })
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::WorldEvent;

    #[tokio::test]
    async fn test_immediate_response_sends_intent() {
        let (intent_tx, mut intent_rx) = mpsc::channel(32);
        let maker = Arc::new(RuleBasedImmediateDecisionMaker::new());
        let handler = ImmediateEventHandler::new(maker, intent_tx);

        handler.set_tick_id(10).await;

        // 创建被呼唤的事件
        let event = ServerMessage::ImmediateEvent {
            event_id: Uuid::new_v4(),
            event: WorldEvent {
                event_type: WorldEventType::PublicMessage,
                tick_id: 10,
                description: "有人在呼唤".to_string(),
                metadata: serde_json::json!({
                    "from_agent_id": Uuid::new_v4().to_string(),
                    "content": "喂，那位侠客！",
                }),
            },
            deadline_ms: 0,
        };

        handler.handle_server_message(event).await;

        // 验证发送了 Intent
        let intent = intent_rx.recv().await.unwrap();
        match intent {
            ClientMessage::Intent {
                tick_id,
                action_type,
                action_data,
                priority,
                ..
            } => {
                assert_eq!(tick_id, 10);
                assert_eq!(action_type, "speak");
                assert_eq!(priority, 10);
                assert_eq!(
                    action_data.unwrap().get("content").unwrap().as_str().unwrap(),
                    "何事？"
                );
            }
            _ => panic!("Expected Intent"),
        }
    }

    #[tokio::test]
    async fn test_conflict_detection() {
        let (intent_tx, _intent_rx) = mpsc::channel(32);
        let maker = Arc::new(RuleBasedImmediateDecisionMaker::new());
        let handler = ImmediateEventHandler::new(maker, intent_tx);

        // 设置当前正在移动
        handler.set_current_intent(Some("move".to_string())).await;

        let event = create_test_event("喂！");

        let decision = handler.decision_maker.decide_response(
            &event,
            Some("move"),
            &[],
        );

        match decision {
            Some(ResponseDecision::DeferToMainTick { .. }) => {}
            _ => panic!("Expected DeferToMainTick when moving"),
        }
    }

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
}
