// ============================================================================
// WebSocket 协议消息定义
// ============================================================================
//
// Agent 与外部调度器（OpenClaw）之间的通信协议
//
// 下行（Agent → 外部调度器）：
// - tick: 推送 WorldState + 截止时间
// - tick_closed: 超时通知
//
// 上行（外部调度器 → Agent）：
// - intent: 提交意图
// ============================================================================

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::models::WorldState;

// ============================================================================
// Server 错误码
// ============================================================================

/// 结构化 Server 错误码
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerErrorCode {
    /// Agent 已死亡
    AgentDead,
    /// 速率限制
    RateLimited,
    /// Tick 已过期
    TickExpired,
    /// 重复提交（同一 tick 已提交过意图）
    DuplicateSubmission,
    /// 无效动作
    InvalidAction,
    /// 验证失败
    ValidationFailed,
    /// 未知错误
    Unknown,
}

// ============================================================================
// 下行消息（Agent → 外部调度器）
// ============================================================================

/// 下行消息类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum DownstreamMessage {
    /// Tick 开始通知（每个 Tick 推送）
    Tick {
        /// 当前 Tick ID
        tick_id: i64,
        /// Tick 截止时间（Unix timestamp, 毫秒）
        deadline_ms: u64,
        /// 世界状态
        state: WorldState,
        /// 叙事化上下文（Markdown 格式，供 LLM 推理使用）
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<String>,
        /// 认知上下文（结构化 JSON，引导 OpenClaw 按阶段推理）
        /// 包含：Perception → Motivation → Planning → Decision
        #[serde(skip_serializing_if = "Option::is_none")]
        cognitive_context: Option<crate::infra::api::cognitive_context::CognitiveContext>,
    },

    /// Tick 关闭通知（超时未收到 Intent 时发送）
    TickClosed {
        /// 关闭的 Tick ID
        tick_id: i64,
        /// 关闭原因
        reason: String,
        /// 下一个 Tick 预计多久后开始（毫秒）
        next_tick_in_ms: u64,
    },

    /// 审核请求（发送给 Observer Agent）
    ReviewRequest {
        /// 目标 Tick ID
        tick_id: i64,
        /// 玩家意图
        player_intent: WsPlayerIntent,
        /// 人设摘要
        persona_summary: PersonaSummary,
        /// 世界上下文
        world_context: String,
        /// 审核截止时间（Unix timestamp, 毫秒）
        deadline_ms: u64,
    },

    // === Server 消息透传 ===
    /// Server 错误消息
    ServerError {
        /// 结构化错误码
        code: ServerErrorCode,
        /// 人类可读的错误消息
        message: String,
        /// 关联的 Tick ID（可选）
        #[serde(skip_serializing_if = "Option::is_none")]
        tick_id: Option<i64>,
        /// 当前 Tick ID（帮助客户端同步）
        #[serde(skip_serializing_if = "Option::is_none")]
        current_tick: Option<i64>,
    },

    /// Server 转发对话消息
    ServerDialogue {
        /// 对话类型: request, accept, reject, content, end
        dialogue_type: String,
        /// 发起者 Agent ID
        from_agent_id: Uuid,
        /// 目标 Agent ID（可选）
        #[serde(skip_serializing_if = "Option::is_none")]
        to_agent_id: Option<Uuid>,
        /// 会话 ID（可选）
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// 开场白（request 时有值）
        #[serde(skip_serializing_if = "Option::is_none")]
        opening_remark: Option<String>,
        /// 对话内容（content 时有值）
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },

    /// Server 游戏规则热更新
    ServerGameRulesUpdate {
        /// Tick 持续时间（秒）
        tick_duration_secs: u64,
        /// 规则版本
        version: String,
        /// 最后更新时间
        last_updated: String,
    },

    /// Server 世界观规则热更新
    ServerWorldBuildingRulesUpdate {
        /// 规则版本
        version: String,
        /// 最后更新时间
        last_updated: String,
    },

    /// 消息丢失通知（Lagged 恢复）
    MissedMessages {
        /// 丢失的消息数量
        count: u64,
        /// 是否建议重新同步
        suggest_resync: bool,
    },

    /// LLM 响应（OpenClaw -> Agent，用于 Claw 模式）
    LLMResponse {
        /// 请求 ID（用于匹配请求）
        request_id: String,
        /// LLM 生成内容
        content: String,
        /// 错误信息（可选）
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Agent 死亡通知（Server -> Agent -> OpenClaw）
    /// 通知 OpenClaw 角色已死亡，需要进行转生处理
    AgentDied {
        /// 死亡的 Agent ID
        agent_id: Uuid,
        /// 死亡原因代码（来自配置：hunger, thirst, environmental, combat, etc.）
        cause: String,
        /// 死亡描述（来自配置，叙事化文本）
        description: String,
        /// 死亡位置（node_id）
        location: String,
        /// 当前 tick
        tick_id: i64,
        /// 死亡时间戳（Unix timestamp, 毫秒）
        died_at: i64,
        /// 重生等待时间（tick 数，0 = 立即，-1 = 不可重生）
        rebirth_delay_ticks: i32,
    },
}

/// 玩家意图（用于审核请求）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsPlayerIntent {
    /// 动作类型
    pub action_type: String,
    /// 动作数据
    #[serde(default)]
    pub action_data: Option<Value>,
    /// 思考日志
    #[serde(default)]
    pub thought_log: Option<String>,
}

/// 人设摘要（用于审核请求）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaSummary {
    /// 角色名称
    pub name: String,
    /// 性格特点
    #[serde(default)]
    pub personality: Vec<String>,
    /// 价值观
    #[serde(default)]
    pub values: Vec<String>,
}

// ============================================================================
// 上行消息（外部调度器 → Agent）
// ============================================================================

/// 上行消息类型
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpstreamMessage {
    /// 意图提交
    Intent {
        /// 目标 Tick ID
        tick_id: i64,
        /// 动作类型
        action_type: String,
        /// 动作数据
        #[serde(default)]
        action_data: Option<Value>,
        /// 思考日志（可选）
        #[serde(default)]
        thought_log: Option<String>,
    },

    /// 审核结果（由 Observer Agent 发送）
    ReviewResult {
        /// 目标 Tick ID
        tick_id: i64,
        /// 审核决定
        decision: ReviewDecision,
        /// 审核原因
        #[serde(default)]
        reason: Option<String>,
        /// 叙事化描述（如果通过）
        #[serde(default)]
        narrative: Option<String>,
    },

    /// LLM 请求（Agent -> OpenClaw，用于 Claw 模式）
    LLMRequest {
        /// 请求 ID（用于匹配响应）
        request_id: String,
        /// LLM 提示词
        prompt: String,
    },
}

/// 审核决定
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// 通过
    Approved,
    /// 拒绝
    Rejected,
    /// 需要修改
    NeedsModification,
}

// ============================================================================
// WebSocket 意图（内部使用）
// ============================================================================

/// WebSocket 意图请求
#[derive(Debug, Clone)]
pub struct WsIntent {
    /// 目标 Tick ID
    pub tick_id: i64,
    /// 动作类型
    pub action_type: String,
    /// 动作数据
    pub action_data: Option<Value>,
    /// 思考日志
    pub thought_log: Option<String>,
}

impl From<UpstreamMessage> for Option<WsIntent> {
    fn from(msg: UpstreamMessage) -> Self {
        match msg {
            UpstreamMessage::Intent {
                tick_id,
                action_type,
                action_data,
                thought_log,
            } => Some(WsIntent {
                tick_id,
                action_type,
                action_data,
                thought_log,
            }),
            // ReviewResult 不是 Intent，返回 None
            UpstreamMessage::ReviewResult { .. } => None,
            // LLMRequest 不是 Intent，返回 None
            UpstreamMessage::LLMRequest { .. } => None,
        }
    }
}

// ============================================================================
// ServerMessage 转换函数
// ============================================================================

use cyber_jianghu_protocol::{DialogueMessage, ServerMessage};

impl DownstreamMessage {
    /// 从 ServerMessage 转换为 DownstreamMessage
    ///
    /// 返回 None 表示该消息类型不需要透传（如 WorldState 已通过 Tick 处理）
    pub fn from_server_message(msg: ServerMessage, current_tick: i64) -> Option<Self> {
        match msg {
            ServerMessage::Error { code, message, current_tick_id: server_tick_id } => {
                let resolved_code = Self::resolve_error_code(&code);
                let tick_id = server_tick_id.or_else(|| Self::parse_tick_id(&message));
                Some(DownstreamMessage::ServerError {
                    code: resolved_code,
                    message,
                    tick_id,
                    current_tick: Some(current_tick),
                })
            }
            ServerMessage::Dialogue { message } => match message {
                DialogueMessage::Request {
                    from_agent_id,
                    to_agent_id,
                    opening_remark,
                } => Some(DownstreamMessage::ServerDialogue {
                    dialogue_type: "request".to_string(),
                    from_agent_id,
                    to_agent_id: Some(to_agent_id),
                    session_id: None,
                    opening_remark: Some(opening_remark),
                    content: None,
                }),
                DialogueMessage::Accept {
                    from_agent_id,
                    session_id,
                } => Some(DownstreamMessage::ServerDialogue {
                    dialogue_type: "accept".to_string(),
                    from_agent_id,
                    to_agent_id: None,
                    session_id: Some(session_id),
                    opening_remark: None,
                    content: None,
                }),
                DialogueMessage::Reject {
                    from_agent_id,
                    session_id,
                    reason,
                } => Some(DownstreamMessage::ServerDialogue {
                    dialogue_type: "reject".to_string(),
                    from_agent_id,
                    to_agent_id: None,
                    session_id: Some(session_id),
                    opening_remark: reason,
                    content: None,
                }),
                DialogueMessage::Content {
                    from_agent_id,
                    session_id,
                    content,
                } => Some(DownstreamMessage::ServerDialogue {
                    dialogue_type: "content".to_string(),
                    from_agent_id,
                    to_agent_id: None,
                    session_id: Some(session_id),
                    opening_remark: None,
                    content: Some(content),
                }),
                DialogueMessage::End {
                    from_agent_id,
                    session_id,
                } => Some(DownstreamMessage::ServerDialogue {
                    dialogue_type: "end".to_string(),
                    from_agent_id,
                    to_agent_id: None,
                    session_id: Some(session_id),
                    opening_remark: None,
                    content: None,
                }),
            },
            ServerMessage::GameRulesUpdate { game_rules } => {
                Some(DownstreamMessage::ServerGameRulesUpdate {
                    tick_duration_secs: game_rules.tick_duration_secs,
                    version: game_rules.version,
                    last_updated: game_rules.last_updated,
                })
            }
            ServerMessage::WorldBuildingRulesUpdate { rules } => {
                Some(DownstreamMessage::ServerWorldBuildingRulesUpdate {
                    version: rules.version,
                    last_updated: rules.last_updated,
                })
            }
            ServerMessage::AgentDied {
                agent_id,
                cause,
                description,
                location,
                tick_id,
                died_at,
                rebirth_delay_ticks,
            } => Some(DownstreamMessage::AgentDied {
                agent_id,
                cause,
                description,
                location,
                tick_id,
                died_at,
                rebirth_delay_ticks,
            }),
            // 其他消息类型不透传
            _ => None,
        }
    }

    /// 将 server 发来的结构化错误码映射为本地枚举
    fn resolve_error_code(code: &str) -> ServerErrorCode {
        use cyber_jianghu_protocol::*;
        match code {
            ERROR_CODE_TICK_MISMATCH => ServerErrorCode::TickExpired,
            ERROR_CODE_NOT_ACCEPTING => ServerErrorCode::TickExpired,
            ERROR_CODE_AGENT_DEAD => ServerErrorCode::AgentDead,
            ERROR_CODE_RATE_LIMITED => ServerErrorCode::RateLimited,
            ERROR_CODE_INVALID_MESSAGE => ServerErrorCode::InvalidAction,
            ERROR_CODE_ACTION_FAILED => ServerErrorCode::InvalidAction,
            _ => ServerErrorCode::Unknown,
        }
    }

    /// 从消息中提取 tick_id
    fn parse_tick_id(message: &str) -> Option<i64> {
        // 尝试匹配 "tick_id 100" 或 "tick_id: 100" 或 "tick 100"
        let patterns = [r"tick_id[:\s]+(\d+)", r"tick[:\s]+(\d+)"];

        for pattern in patterns {
            if let Ok(re) = regex::Regex::new(pattern)
                && let Some(caps) = re.captures(message)
                && let Some(m) = caps.get(1)
                && let Ok(n) = m.as_str().parse::<i64>()
            {
                return Some(n);
            }
        }
        None
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_jianghu_protocol::GameRules;

    fn create_test_world_state() -> WorldState {
        // 使用 JSON 构造测试数据，避免直接构造复杂结构
        let json = serde_json::json!({
            "event_type": "world_state",
            "tick_id": 105,
            "agent_id": "00000000-0000-0000-0000-000000000000",
            "world_time": {
                "year": 2024,
                "month": 1,
                "day": 1,
                "hour": 12,
                "minute": 0,
                "second": 0,
                "weather": "晴"
            },
            "location": {
                "node_id": "test",
                "name": "测试地点",
                "type": "indoor",
                "adjacent_nodes": []
            },
            "self_state": {
                "attributes": {},
                "attribute_descriptions": {},
                "status_effects": []
            },
            "entities": [],
            "nearby_items": [],
            "events_log": [],
            "available_actions": []
        });
        serde_json::from_value(json).unwrap()
    }

    // === 新增消息类型测试 ===

    #[test]
    fn test_serialize_server_error_agent_dead() {
        let msg = DownstreamMessage::ServerError {
            code: ServerErrorCode::AgentDead,
            message: "Agent 已死亡，无法执行此动作。".to_string(),
            tick_id: Some(105),
            current_tick: Some(110),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"server_error""#));
        assert!(json.contains(r#""code":"agent_dead""#));
        assert!(json.contains(r#""tick_id":105"#));
        assert!(json.contains(r#""current_tick":110"#));
    }

    #[test]
    fn test_serialize_server_error_rate_limited() {
        let msg = DownstreamMessage::ServerError {
            code: ServerErrorCode::RateLimited,
            message: "Rate limit exceeded.".to_string(),
            tick_id: None,
            current_tick: Some(100),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"server_error""#));
        assert!(json.contains(r#""code":"rate_limited""#));
        assert!(!json.contains(r#""tick_id""#)); // None 时不序列化
        assert!(json.contains(r#""current_tick":100"#));
    }

    #[test]
    fn test_serialize_server_dialogue_request() {
        let msg = DownstreamMessage::ServerDialogue {
            dialogue_type: "request".to_string(),
            from_agent_id: Uuid::nil(),
            to_agent_id: Some(Uuid::nil()),
            session_id: None,
            opening_remark: Some("少侠，可否借一步说话？".to_string()),
            content: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"server_dialogue""#));
        assert!(json.contains(r#""dialogue_type":"request""#));
        assert!(json.contains(r#""opening_remark""#));
    }

    #[test]
    fn test_serialize_server_game_rules_update() {
        let msg = DownstreamMessage::ServerGameRulesUpdate {
            tick_duration_secs: 60,
            version: "0.0.5".to_string(),
            last_updated: "2024-03-22T10:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"server_game_rules_update""#));
        assert!(json.contains(r#""tick_duration_secs":60"#));
    }

    #[test]
    fn test_serialize_missed_messages() {
        let msg = DownstreamMessage::MissedMessages {
            count: 3,
            suggest_resync: false,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"missed_messages""#));
        assert!(json.contains(r#""count":3"#));
        assert!(json.contains(r#""suggest_resync":false"#));
    }

    // === 原有测试 ===

    #[test]
    fn test_serialize_tick_message() {
        let state = create_test_world_state();

        let msg = DownstreamMessage::Tick {
            tick_id: 105,
            deadline_ms: 1710937800000,
            state,
            context: None,
            cognitive_context: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"tick""#));
        assert!(json.contains(r#""tick_id":105"#));
        assert!(!json.contains(r#""context""#)); // None 时不序列化
        assert!(!json.contains(r#""cognitive_context""#)); // None 时不序列化
    }

    #[test]
    fn test_serialize_tick_message_with_context() {
        use crate::infra::api::cognitive_context::{
            CognitiveContext, DecisionContext, MotivationContext, PerceptionContext,
            PlanningContext,
        };

        let state = create_test_world_state();

        // 创建结构化认知上下文
        let cognitive_context = CognitiveContext {
            perception: PerceptionContext {
                self_status: "身体状态良好".to_string(),
                environment: "长安城东市".to_string(),
                key_observations: vec!["附近有商人".to_string()],
            },
            motivation: MotivationContext {
                active_drives: vec![],
                dominant_drive: "保持现状".to_string(),
            },
            planning: PlanningContext {
                current_goals: vec!["继续当前活动".to_string()],
                available_actions: vec![],
            },
            decision: DecisionContext {
                requires_reasoning: true,
                thinking_prompt: "请决定下一步行动".to_string(),
            },
        };

        let msg = DownstreamMessage::Tick {
            tick_id: 105,
            deadline_ms: 1710937800000,
            state,
            context: Some("## 游戏状态上下文\n\n测试上下文".to_string()),
            cognitive_context: Some(cognitive_context),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"tick""#));
        assert!(json.contains(r#""tick_id":105"#));
        assert!(json.contains(r#""context""#)); // 有 context 字段
        assert!(json.contains(r#""cognitive_context""#)); // 有 cognitive_context 字段
        assert!(json.contains(r#""perception""#));
        assert!(json.contains(r#""motivation""#));
        assert!(json.contains(r#""planning""#));
        assert!(json.contains(r#""decision""#));
    }

    #[test]
    fn test_serialize_tick_closed_message() {
        let msg = DownstreamMessage::TickClosed {
            tick_id: 105,
            reason: "timeout".to_string(),
            next_tick_in_ms: 60000,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"tick_closed""#));
        assert!(json.contains(r#""reason":"timeout""#));
    }

    #[test]
    fn test_deserialize_intent_message() {
        let json = r#"{"type":"intent","tick_id":105,"action_type":"move","action_data":{"target":"kitchen"}}"#;
        let msg: UpstreamMessage = serde_json::from_str(json).unwrap();

        match msg {
            UpstreamMessage::Intent {
                tick_id,
                action_type,
                action_data,
                thought_log,
            } => {
                assert_eq!(tick_id, 105);
                assert_eq!(action_type, "move");
                assert!(action_data.is_some());
                assert!(thought_log.is_none());
            }
            _ => panic!("Expected Intent message"),
        }
    }

    #[test]
    fn test_deserialize_intent_with_thought() {
        let json = r#"{"type":"intent","tick_id":105,"action_type":"speak","thought_log":"I should greet them"}"#;
        let msg: UpstreamMessage = serde_json::from_str(json).unwrap();

        match msg {
            UpstreamMessage::Intent {
                tick_id,
                action_type,
                action_data,
                thought_log,
            } => {
                assert_eq!(tick_id, 105);
                assert_eq!(action_type, "speak");
                assert!(action_data.is_none());
                assert_eq!(thought_log, Some("I should greet them".to_string()));
            }
            _ => panic!("Expected Intent message"),
        }
    }

    #[test]
    fn test_deserialize_review_result_approved() {
        let json = r#"{"type":"review_result","tick_id":105,"decision":"approved","reason":"符合角色性格","narrative":"张三热情地向店小二打招呼"}"#;
        let msg: UpstreamMessage = serde_json::from_str(json).unwrap();

        match msg {
            UpstreamMessage::ReviewResult {
                tick_id,
                decision,
                reason,
                narrative,
            } => {
                assert_eq!(tick_id, 105);
                assert!(matches!(decision, ReviewDecision::Approved));
                assert_eq!(reason, Some("符合角色性格".to_string()));
                assert_eq!(narrative, Some("张三热情地向店小二打招呼".to_string()));
            }
            _ => panic!("Expected ReviewResult message"),
        }
    }

    #[test]
    fn test_deserialize_review_result_rejected() {
        let json = r#"{"type":"review_result","tick_id":105,"decision":"rejected"}"#;
        let msg: UpstreamMessage = serde_json::from_str(json).unwrap();

        match msg {
            UpstreamMessage::ReviewResult {
                tick_id,
                decision,
                reason,
                narrative,
            } => {
                assert_eq!(tick_id, 105);
                assert!(matches!(decision, ReviewDecision::Rejected));
                assert!(reason.is_none());
                assert!(narrative.is_none());
            }
            _ => panic!("Expected ReviewResult message"),
        }
    }

    // === from_server_message 转换测试 ===

    #[test]
    fn test_from_server_message_error() {
        let server_msg = ServerMessage::Error {
            code: cyber_jianghu_protocol::ERROR_CODE_AGENT_DEAD.to_string(),
            message: "Agent 已死亡，无法执行此动作。".to_string(),
            current_tick_id: None,
        };

        let result = DownstreamMessage::from_server_message(server_msg, 100);
        assert!(result.is_some());

        match result.unwrap() {
            DownstreamMessage::ServerError {
                code,
                message,
                tick_id,
                current_tick,
            } => {
                assert_eq!(code, ServerErrorCode::AgentDead);
                assert!(message.contains("死亡"));
                assert!(tick_id.is_none()); // 消息中没有 tick_id
                assert_eq!(current_tick, Some(100)); // 传入的 current_tick
            }
            _ => panic!("Expected ServerError"),
        }
    }

    #[test]
    fn test_from_server_message_error_with_tick() {
        let server_msg = ServerMessage::Error {
            code: String::new(),
            message: "tick 105: invalid action".to_string(),
            current_tick_id: None,
        };

        let result = DownstreamMessage::from_server_message(server_msg, 100);
        assert!(result.is_some());

        match result.unwrap() {
            DownstreamMessage::ServerError {
                code: _,
                message,
                tick_id,
                current_tick,
            } => {
                assert_eq!(tick_id, Some(105));
                assert!(message.contains("tick 105"));
                assert_eq!(current_tick, Some(100)); // 传入的 current_tick
            }
            _ => panic!("Expected ServerError"),
        }
    }

    #[test]
    fn test_from_server_message_dialogue_request() {
        let from_id = Uuid::new_v4();
        let to_id = Uuid::new_v4();

        let server_msg = ServerMessage::Dialogue {
            message: DialogueMessage::Request {
                from_agent_id: from_id,
                to_agent_id: to_id,
                opening_remark: "少侠，可否借一步说话？".to_string(),
            },
        };

        let result = DownstreamMessage::from_server_message(server_msg, 100);
        assert!(result.is_some());

        match result.unwrap() {
            DownstreamMessage::ServerDialogue {
                dialogue_type,
                from_agent_id,
                to_agent_id,
                session_id,
                opening_remark,
                content,
            } => {
                assert_eq!(dialogue_type, "request");
                assert_eq!(from_agent_id, from_id);
                assert_eq!(to_agent_id, Some(to_id));
                assert!(session_id.is_none());
                assert_eq!(opening_remark, Some("少侠，可否借一步说话？".to_string()));
                assert!(content.is_none());
            }
            _ => panic!("Expected ServerDialogue"),
        }
    }

    #[test]
    fn test_from_server_message_dialogue_content() {
        let from_id = Uuid::new_v4();

        let server_msg = ServerMessage::Dialogue {
            message: DialogueMessage::Content {
                from_agent_id: from_id,
                session_id: "session-123".to_string(),
                content: "今天天气不错。".to_string(),
            },
        };

        let result = DownstreamMessage::from_server_message(server_msg, 100);
        assert!(result.is_some());

        match result.unwrap() {
            DownstreamMessage::ServerDialogue {
                dialogue_type,
                from_agent_id,
                to_agent_id,
                session_id,
                opening_remark,
                content,
            } => {
                assert_eq!(dialogue_type, "content");
                assert_eq!(from_agent_id, from_id);
                assert!(to_agent_id.is_none());
                assert_eq!(session_id, Some("session-123".to_string()));
                assert!(opening_remark.is_none());
                assert_eq!(content, Some("今天天气不错。".to_string()));
            }
            _ => panic!("Expected ServerDialogue"),
        }
    }

    #[test]
    fn test_from_server_message_game_rules_update() {
        let server_msg = ServerMessage::GameRulesUpdate {
            game_rules: GameRules {
                tick_duration_secs: 30,
                available_actions: vec![],
                initial_items: vec![],
                survival_actions: vec![],
                survival_threshold: 30,
                version: "0.0.6".to_string(),
                last_updated: "2024-03-22T12:00:00Z".to_string(),
            },
        };

        let result = DownstreamMessage::from_server_message(server_msg, 100);
        assert!(result.is_some());

        match result.unwrap() {
            DownstreamMessage::ServerGameRulesUpdate {
                tick_duration_secs,
                version,
                last_updated,
            } => {
                assert_eq!(tick_duration_secs, 30);
                assert_eq!(version, "0.0.6");
                assert_eq!(last_updated, "2024-03-22T12:00:00Z");
            }
            _ => panic!("Expected ServerGameRulesUpdate"),
        }
    }

    #[test]
    fn test_from_server_message_world_state_skipped() {
        // WorldState 不应该被转换（已有专门的 Tick 处理）
        let server_msg = ServerMessage::WorldState {
            data: create_test_world_state(),
        };

        let result = DownstreamMessage::from_server_message(server_msg, 100);
        assert!(result.is_none());
    }

}
