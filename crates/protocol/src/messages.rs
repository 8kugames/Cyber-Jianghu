//! WebSocket 消息定义
//!
//! 统一的消息格式，确保服务端和客户端兼容。
//!
//! ## 消息流向
//!
//! - [`ServerMessage`] - 服务端 → Agent (注册确认、世界状态、规则更新、错误)
//! - [`ClientMessage`] - Agent → 服务端 (意图上报、心跳、对话)
//!
//! ## 对话系统
//!
//! - [`DialogueMessage`] - Agent 间直接对话 (请求、接受、内容、结束)
//! - [`DialogueSession`] - 服务端维护的对话会话状态

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{AvailableAction, GameRules, WorldBuildingRules, WorldState};

/// serde default helper: 缺省 true（fail-open，旧 server 不发字段时假定存活）
fn default_true() -> bool {
    true
}

// ============================================================================
// 对话消息类型
// ============================================================================

/// 对话消息（Agent 间直接交换）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "message_type", rename_all = "snake_case")]
pub enum DialogueMessage {
    /// 请求建立对话
    Request {
        from_agent_id: Uuid,
        to_agent_id: Uuid,
        opening_remark: String,
    },

    /// 接受对话
    Accept {
        session_id: String,
        from_agent_id: Uuid,
    },

    /// 拒绝对话
    Reject {
        session_id: String,
        from_agent_id: Uuid,
        reason: Option<String>,
    },

    /// 对话内容
    Content {
        session_id: String,
        from_agent_id: Uuid,
        content: String,
    },

    /// 结束对话
    End {
        session_id: String,
        from_agent_id: Uuid,
    },
}

/// 对话会话状态（服务端维护）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogueSession {
    pub session_id: String,
    pub agent_a: Uuid,
    pub agent_b: Uuid,
    pub started_at: DateTime<Utc>,
    pub message_count: u32,
}

// ============================================================================
// 服务端消息
// ============================================================================

/// 服务端下发的消息
///
/// # 消息类型
///
/// - `Registered`: Agent 注册成功，包含游戏规则和世界观规则
/// - `WorldState`: 每个 Tick 下发的完整世界状态快照
/// - `GameRulesUpdate`: 游戏规则热更新通知
/// - `WorldBuildingRulesUpdate`: 世界观规则热更新通知
/// - `Pong`: 心跳响应
/// - `Error`: 错误通知
/// - `Dialogue`: 转发 Agent 间对话消息
///
/// # 示例
///
/// ```rust
/// use cyber_jianghu_protocol::{GameRules, ServerMessage};
/// use uuid::Uuid;
///
/// let game_rules = GameRules {
///     tick_duration_secs: 60,
///     available_actions: vec![],
///     initial_items: vec![],
///     survival_actions: vec![],
///     survival_threshold: 30,
///     version: "0.0.1".to_string(),
///     last_updated: "2024-01-01T00:00:00Z".to_string(),
/// };
///
/// let msg = ServerMessage::Registered {
///     agent_id: Uuid::new_v4(),
///     game_rules,
///     world_building_rules: None,
///     is_alive: true,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// 注册成功（包含游戏规则）
    Registered {
        agent_id: Uuid,
        #[serde(flatten)]
        game_rules: GameRules,
        /// 世界观规则（可选，保持向后兼容）
        #[serde(skip_serializing_if = "Option::is_none")]
        world_building_rules: Option<WorldBuildingRules>,
        /// 角色是否存活（由服务器在连接时立即告知）
        /// 缺省 true：旧 server 不发此字段时假定存活（fail-open）
        #[serde(default = "default_true")]
        is_alive: bool,
        /// 角色名称（可选，首次连接时由服务器填充）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_name: Option<String>,
    },

    /// 世界状态下发
    WorldState {
        #[serde(flatten)]
        data: WorldState,
    },

    /// 游戏规则更新
    GameRulesUpdate {
        #[serde(flatten)]
        game_rules: GameRules,
    },

    /// 世界观规则更新（新增）
    WorldBuildingRulesUpdate { rules: WorldBuildingRules },

    /// 动作配置增量更新
    ///
    /// 仅当下发变更时出现，完整动作列表由 agent 本地缓存。
    /// action_update_type: "full" | "incremental"
    ActionUpdate {
        /// 更新类型
        update_type: String,
        /// 动作列表（全量时有效）
        actions: Vec<AvailableAction>,
        /// 增量更新的动作名称列表（增量时有效）
        updated_actions: Vec<String>,
        /// 被删除的动作名称列表（增量时有效）
        removed_actions: Vec<String>,
        /// 规则版本
        version: String,
    },

    /// 心跳响应
    Pong { timestamp: i64 },

    /// 错误消息
    Error {
        /// 机器可读错误码（如 "tick_mismatch", "agent_dead"）
        /// 详见 `crate::ERROR_CODE_*` 常量
        #[serde(default, skip_serializing_if = "String::is_empty")]
        code: String,
        /// 人类可读错误描述
        message: String,
        /// tick 不匹配时的当前 tick_id（仅 tick_mismatch 有值）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current_tick_id: Option<i64>,
    },

    /// 对话消息（转发）
    Dialogue {
        #[serde(flatten)]
        message: DialogueMessage,
    },

    /// Agent 死亡通知
    ///
    /// 当 Agent 因任何原因死亡时，Server 立即推送此消息。
    /// Agent 收到后透传给 OpenClaw，触发重生流程。
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

// ============================================================================
// 客户端消息
// ============================================================================

/// Agent 上报的消息
///
/// # 消息类型
///
/// - `Intent`: Agent 意图上报 (每 Tick 一次，包含动作类型和参数)
/// - `Ping`: 心跳请求
/// - `Dialogue`: Agent 间对话消息 (通过服务端转发)
///
/// # 意图上报示例
///
/// ```rust
/// use cyber_jianghu_protocol::ClientMessage;
/// use serde_json::json;
///
/// let msg = ClientMessage::Intent {
///     intent_id: None,
///     tick_id: 1,
///     agent_id: None,
///     thought_log: Some("思考过程".to_string()),
///     action_type: "speak".to_string(),
///     action_data: Some(json!({"content": "你好"})),
///     priority: 5,
/// };
/// ```
///
/// 与服务器端格式保持一致，使用扁平化字段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// 意图上报
    Intent {
        /// Intent 唯一 ID（可选，如果未提供则服务端自动生成）
        #[serde(skip_serializing_if = "Option::is_none")]
        intent_id: Option<Uuid>,
        /// Tick 编号
        tick_id: i64,
        /// Agent ID（可选，不提供则使用连接关联的 agent）
        /// 用于支持同一设备上的多角色切换
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_id: Option<Uuid>,
        /// 思考日志
        #[serde(skip_serializing_if = "Option::is_none")]
        thought_log: Option<String>,
        /// 动作类型
        action_type: String,
        /// 动作参数
        #[serde(skip_serializing_if = "Option::is_none")]
        action_data: Option<serde_json::Value>,
        /// 优先级
        priority: i32,
    },

    /// 对话消息
    Dialogue {
        #[serde(flatten)]
        message: DialogueMessage,
    },
}

// ============================================================================
// 消息解析辅助
// ============================================================================

impl ClientMessage {
    /// 从 JSON 字符串解析
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// 转换为 JSON 字符串
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// 从 Intent 创建 ClientMessage
    pub fn from_intent(intent: crate::types::Intent) -> Self {
        ClientMessage::Intent {
            intent_id: Some(intent.intent_id),
            tick_id: intent.tick_id,
            agent_id: Some(intent.agent_id),
            thought_log: intent.thought_log,
            action_type: intent.action_type.to_string(),
            action_data: intent.action_data,
            priority: intent.priority,
        }
    }

    /// 从 DialogueMessage 创建 ClientMessage
    pub fn from_dialogue(message: DialogueMessage) -> Self {
        ClientMessage::Dialogue { message }
    }
}

impl ServerMessage {
    /// 从 JSON 字符串解析
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// 转换为 JSON 字符串
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Intent;

    #[test]
    fn test_client_message_serialization() {
        let agent_id = Uuid::nil();
        let intent = Intent::new(agent_id, 1, "idle", None);
        let msg = ClientMessage::from_intent(intent);

        let json = msg.to_json().unwrap();
        println!("Serialized ClientMessage: {}", json);

        // 验证格式 - 应该是扁平化的
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "intent");
        assert_eq!(parsed["tick_id"], 1);
        assert_eq!(parsed["action_type"], "idle");
    }

    #[test]
    fn test_client_message_deserialization() {
        // 使用扁平化格式
        let json = r#"{"type":"intent","tick_id":1,"action_type":"speak","action_data":{"content":"hello"},"priority":5}"#;

        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Intent {
                tick_id,
                action_type,
                action_data: _,
                priority,
                ..
            } => {
                assert_eq!(tick_id, 1);
                assert_eq!(action_type, "speak");
                assert_eq!(priority, 5);
            }
            _ => panic!("Unexpected message type"),
        }
    }

    #[test]
    fn test_server_message_registered() {
        let agent_id = Uuid::nil();
        let game_rules = GameRules {
            tick_duration_secs: 60,
            initial_items: vec![],
            survival_actions: vec![],
            available_actions: vec![],
            survival_threshold: 30,
            version: "0.0.1".to_string(),
            last_updated: "2024-01-01T00:00:00Z".to_string(),
        };
        let msg = ServerMessage::Registered {
            agent_id,
            game_rules,
            world_building_rules: None,
            is_alive: true,
            agent_name: None,
        };

        let json = msg.to_json().unwrap();
        println!("Serialized Registered: {}", json);

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "registered");
        assert_eq!(parsed["agent_id"], "00000000-0000-0000-0000-000000000000");
        assert_eq!(parsed["tick_duration_secs"], 60);
    }

    #[test]
    fn test_server_message_world_state() {
        let world_state = WorldState {
            event_type: "world_state".to_string(),
            tick_id: 1,
            agent_id: None,
            world_time: crate::types::WorldTime {
                year: 2024,
                month: 3,
                day: 15,
                hour: 12,
                minute: 0,
                second: 0,
                weather: "晴".to_string(),
            },
            location: crate::types::Location {
                node_id: "test".to_string(),
                name: "Test".to_string(),
                node_type: "客栈".to_string(),
                adjacent_nodes: vec![],
            },
            self_state: crate::types::AgentSelfState {
                attributes: {
                    let mut attrs = std::collections::HashMap::new();
                    attrs.insert("hp".to_string(), 100);
                    attrs.insert("stamina".to_string(), 100);
                    attrs.insert("hunger".to_string(), 50);
                    attrs.insert("thirst".to_string(), 50);
                    attrs
                },
                derived_attributes: std::collections::HashMap::new(),
                attribute_descriptions: std::collections::HashMap::new(),
                status_effects: vec![],
                inventory: vec![],
            },
            entities: vec![],
            nearby_items: vec![],
            events_log: vec![],
            deadline_ms: 0,
        };

        let msg = ServerMessage::WorldState { data: world_state };
        let json = msg.to_json().unwrap();
        println!("ServerMessage WorldState: {}", json);

        // 验证 flatten 效果
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "world_state");
        assert_eq!(parsed["tick_id"], 1);
    }

    #[test]
    fn test_server_message_pong() {
        let msg = ServerMessage::Pong {
            timestamp: 1234567890,
        };
        let json = msg.to_json().unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "pong");
        assert_eq!(parsed["timestamp"], 1234567890);
    }

    #[test]
    fn test_server_message_error() {
        let msg = ServerMessage::Error {
            code: "unknown".to_string(),
            message: "Something went wrong".to_string(),
            current_tick_id: None,
        };
        let json = msg.to_json().unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "error");
        assert_eq!(parsed["message"], "Something went wrong");
        assert_eq!(parsed["code"], "unknown");
    }

    #[test]
    fn test_server_message_error_no_code() {
        // 不带 code 字段时默认为空字符串
        let json = r#"{"type":"error","message":"Something went wrong"}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::Error {
                code,
                message,
                current_tick_id: _,
            } => {
                assert!(code.is_empty());
                assert_eq!(message, "Something went wrong");
            }
            _ => panic!("Expected Error"),
        }
    }

    #[test]
    fn test_dialogue_message_serialization() {
        let msg = DialogueMessage::Request {
            from_agent_id: Uuid::new_v4(),
            to_agent_id: Uuid::new_v4(),
            opening_remark: "你好，能聊聊吗？".to_string(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("request"));

        let parsed: DialogueMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            DialogueMessage::Request { opening_remark, .. } => {
                assert_eq!(opening_remark, "你好，能聊聊吗？");
            }
            _ => panic!("Unexpected message type"),
        }
    }

    #[test]
    fn test_server_message_world_building_rules_update() {
        use crate::types::WorldBuildingRules;

        let rules = WorldBuildingRules::default();
        let msg = ServerMessage::WorldBuildingRulesUpdate { rules };

        let json = msg.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "world_building_rules_update");
        assert!(parsed.get("rules").is_some());
    }

    #[test]
    fn test_server_message_registered_with_world_building_rules() {
        use crate::types::WorldBuildingRules;

        let agent_id = Uuid::nil();
        let game_rules = GameRules {
            tick_duration_secs: 60,
            initial_items: vec![],
            survival_actions: vec![],
            available_actions: vec![],
            survival_threshold: 30,
            version: "0.0.1".to_string(),
            last_updated: "2024-01-01T00:00:00Z".to_string(),
        };
        let world_rules = WorldBuildingRules::default();

        let msg = ServerMessage::Registered {
            agent_id,
            game_rules,
            world_building_rules: Some(world_rules),
            is_alive: true,
            agent_name: None,
        };

        let json = msg.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "registered");
        assert!(parsed.get("world_building_rules").is_some());
    }

    #[test]
    fn test_server_message_agent_died() {
        let msg = ServerMessage::AgentDied {
            agent_id: Uuid::nil(),
            cause: "hunger".to_string(),
            description: "因饥饿而死".to_string(),
            location: "tavern".to_string(),
            tick_id: 42,
            died_at: 1234567890000,
            rebirth_delay_ticks: 10,
        };

        let json = msg.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Verify type is serialized as "agent_died" (snake_case)
        assert_eq!(parsed["type"], "agent_died");
        assert_eq!(parsed["agent_id"], "00000000-0000-0000-0000-000000000000");
        assert_eq!(parsed["cause"], "hunger");
        assert_eq!(parsed["description"], "因饥饿而死");
        assert_eq!(parsed["location"], "tavern");
        assert_eq!(parsed["tick_id"], 42);
        assert_eq!(parsed["died_at"], 1234567890000_i64);
        assert_eq!(parsed["rebirth_delay_ticks"], 10);

        // Verify round-trip deserialization
        let deserialized: ServerMessage = ServerMessage::from_json(&json).unwrap();
        match deserialized {
            ServerMessage::AgentDied {
                agent_id,
                cause,
                description,
                location,
                tick_id,
                died_at,
                rebirth_delay_ticks,
            } => {
                assert_eq!(agent_id, Uuid::nil());
                assert_eq!(cause, "hunger");
                assert_eq!(description, "因饥饿而死");
                assert_eq!(location, "tavern");
                assert_eq!(tick_id, 42);
                assert_eq!(died_at, 1234567890000);
                assert_eq!(rebirth_delay_ticks, 10);
            }
            _ => panic!("Unexpected message type"),
        }
    }
}
