// ============================================================================
// WebSocket 协议消息定义
// ============================================================================
//
// Agent 与外部调度器（OpenClaw）之间的通信协议
//
// 下行（Agent → 外部调度器）：
// - tick: 推送 WorldState
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

    /// 审核请求（ReflectorSoul 外部审查）
    ReviewRequest {
        /// 目标 Tick ID
        tick_id: i64,
        /// 玩家意图
        player_intent: WsPlayerIntent,
        /// 人设摘要
        persona_summary: PersonaSummary,
        /// 世界上下文
        world_context: String,
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
        /// 死亡原因代码（来自配置：satiation, hydration, environmental, combat, etc.）
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
        /// 死亡上下文（属性快照等，供同地 Agent 学习）
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },

    /// Server 即时事件（speak 广播等实时推送）
    ServerImmediateEvent {
        /// 事件类型
        event_type: String,
        /// Tick ID
        tick_id: i64,
        /// 事件描述
        description: String,
        /// 事件元数据
        metadata: Value,
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

/// 上游原子意图（UpstreamMessage::Intent 的队列元素）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpstreamIntent {
    /// 动作类型
    pub action_type: String,
    /// 动作数据
    #[serde(default)]
    pub action_data: Option<Value>,
    /// 思考日志（可选）
    #[serde(default)]
    pub thought_log: Option<String>,
}

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
        /// 原子意图队列（可选；multi-Intent 支持，缺省为空保证旧上游兼容）
        #[serde(default)]
        subsequent_intents: Vec<UpstreamIntent>,
    },

    /// 审核结果（ReflectorSoul 审查返回）
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
    /// 原子意图队列（multi-Intent 支持）
    pub subsequent_intents: Vec<WsIntent>,
}

impl From<UpstreamMessage> for Option<WsIntent> {
    fn from(msg: UpstreamMessage) -> Self {
        match msg {
            UpstreamMessage::Intent {
                tick_id,
                action_type,
                action_data,
                thought_log,
                subsequent_intents,
            } => Some(WsIntent {
                tick_id,
                action_type,
                action_data,
                thought_log,
                subsequent_intents: subsequent_intents
                    .into_iter()
                    .map(|ui| WsIntent {
                        tick_id,
                        action_type: ui.action_type,
                        action_data: ui.action_data,
                        thought_log: ui.thought_log,
                        subsequent_intents: Vec::new(),
                    })
                    .collect(),
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
            ServerMessage::Error {
                code,
                message,
                current_tick_id: server_tick_id,
            } => {
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
            ServerMessage::ConfigUpdate {
                config_type,
                version: _,
                content,
                ..
            } => match config_type {
                cyber_jianghu_protocol::ConfigType::GameRules => {
                    if let Ok(game_rules) =
                        serde_json::from_value::<cyber_jianghu_protocol::GameRules>(content.clone())
                    {
                        Some(DownstreamMessage::ServerGameRulesUpdate {
                            tick_duration_secs: game_rules.tick_duration_secs,
                            version: game_rules.version,
                            last_updated: game_rules.last_updated,
                        })
                    } else {
                        None
                    }
                }
                cyber_jianghu_protocol::ConfigType::WorldBuildingRules => {
                    if let Ok(rules) = serde_json::from_value::<
                        cyber_jianghu_protocol::WorldBuildingRules,
                    >(content.clone())
                    {
                        Some(DownstreamMessage::ServerWorldBuildingRulesUpdate {
                            version: rules.version,
                            last_updated: rules.last_updated,
                        })
                    } else {
                        None
                    }
                }
                cyber_jianghu_protocol::ConfigType::Skills
                | cyber_jianghu_protocol::ConfigType::Actions
                | cyber_jianghu_protocol::ConfigType::PromptTemplates
                | cyber_jianghu_protocol::ConfigType::PersonaEventRules
                | cyber_jianghu_protocol::ConfigType::NarrativeConfig => None,
            },
            ServerMessage::AgentDied {
                agent_id,
                cause,
                description,
                location,
                tick_id,
                died_at,
                rebirth_delay_ticks,
                metadata,
            } => Some(DownstreamMessage::AgentDied {
                agent_id,
                cause,
                description,
                location,
                tick_id,
                died_at,
                rebirth_delay_ticks,
                metadata,
            }),
            ServerMessage::ImmediateEvent { event_id: _, event } => {
                Some(DownstreamMessage::ServerImmediateEvent {
                    event_type: event.event_type.to_string(),
                    tick_id: event.tick_id,
                    description: event.description,
                    metadata: event.metadata,
                })
            }
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
#[path = "protocol_tests.rs"]
mod tests;
