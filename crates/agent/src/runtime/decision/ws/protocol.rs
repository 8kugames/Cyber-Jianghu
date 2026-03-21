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

use crate::models::WorldState;

// ============================================================================
// 下行消息（Agent → 外部调度器）
// ============================================================================

/// 下行消息类型
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
#[derive(Debug, Clone, Deserialize)]
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
        }
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_serialize_tick_message() {
        let state = create_test_world_state();

        let msg = DownstreamMessage::Tick {
            tick_id: 105,
            deadline_ms: 1710937800000,
            state,
            context: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"tick""#));
        assert!(json.contains(r#""tick_id":105"#));
        assert!(!json.contains(r#""context""#)); // None 时不序列化
    }

    #[test]
    fn test_serialize_tick_message_with_context() {
        let state = create_test_world_state();

        let msg = DownstreamMessage::Tick {
            tick_id: 105,
            deadline_ms: 1710937800000,
            state,
            context: Some("## 游戏状态上下文\n\n测试上下文".to_string()),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"tick""#));
        assert!(json.contains(r#""tick_id":105"#));
        assert!(json.contains(r#""context""#)); // 有 context 字段
    }

    #[test]
    fn test_serialize_tick_closed_message() {
        let msg = DownstreamMessage::TickClosed {
            tick_id: 105,
            reason: "timeout".to_string(),
            next_tick_in_ms: 15000,
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
}
