//! 动作和意图相关类型
//!
//! 数据驱动设计：ActionType 是字符串，不限制具体值
//! 可用动作类型由 WorldState.available_actions 动态提供

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;
use uuid::Uuid;

/// 动作类型 - 完全数据驱动
///
/// 不再使用枚举，而是使用字符串包装类型。
/// 具体可用动作从 WorldState.available_actions 获取。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(transparent)]
pub struct ActionType(String);

impl ActionType {
    /// 创建新的动作类型
    pub fn new(action: impl Into<String>) -> Self {
        Self(action.into())
    }

    /// 获取动作类型字符串
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 常用动作类型常量（便捷访问）
    pub const IDLE: &'static str = "idle";
    pub const SPEAK: &'static str = "speak";
    pub const MOVE: &'static str = "move";
    pub const GIVE: &'static str = "give";
    pub const STEAL: &'static str = "steal";
    pub const USE: &'static str = "use";
    pub const PICKUP: &'static str = "pickup";
    pub const ATTACK: &'static str = "attack";
    pub const TRADE: &'static str = "trade";
    pub const DROP: &'static str = "drop";
    pub const GATHER: &'static str = "gather";
    pub const CRAFT: &'static str = "craft";
}

impl Default for ActionType {
    fn default() -> Self {
        Self(Self::IDLE.to_string())
    }
}

impl fmt::Display for ActionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ActionType {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ActionType {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<ActionType> for String {
    fn from(action: ActionType) -> Self {
        action.0
    }
}

impl Deref for ActionType {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for ActionType {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for ActionType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

// ============================================================================
// 意图
// ============================================================================

/// Agent 上报的意图
///
/// 每个 Tick，Agent 通过 WebSocket 上报意图，包含要执行的动作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    /// Intent 唯一 ID，用于全链路追踪
    #[serde(default = "uuid::Uuid::new_v4")]
    pub intent_id: Uuid,

    /// Agent ID
    pub agent_id: Uuid,

    /// Tick 编号
    pub tick_id: i64,

    /// 思考日志（Agent 的内心独白）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_log: Option<String>,

    /// 动作类型（数据驱动，任意字符串）
    pub action_type: ActionType,

    /// 动作参数（JSON 格式）
    ///
    /// 不同动作类型的参数由服务端配置定义
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_data: Option<serde_json::Value>,

    /// 优先级（1-10，1 最高）
    #[serde(default = "default_priority")]
    pub priority: i32,
}

fn default_priority() -> i32 {
    5
}

impl Intent {
    /// 创建通用意图（数据驱动）
    pub fn new(
        agent_id: Uuid,
        tick_id: i64,
        action_type: impl Into<ActionType>,
        action_data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            intent_id: Uuid::new_v4(),
            agent_id,
            tick_id,
            thought_log: None,
            action_type: action_type.into(),
            action_data,
            priority: 5,
        }
    }

    /// 创建带 intent_id 的通用意图（数据驱动）
    pub fn new_with_id(
        intent_id: Uuid,
        agent_id: Uuid,
        tick_id: i64,
        action_type: impl Into<ActionType>,
        action_data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            intent_id,
            agent_id,
            tick_id,
            thought_log: None,
            action_type: action_type.into(),
            action_data,
            priority: 5,
        }
    }

    /// 创建 idle 意图
    pub fn idle(agent_id: Uuid, tick_id: i64) -> Self {
        Self::new(agent_id, tick_id, ActionType::IDLE, None)
    }

    /// 创建 speak 意图
    pub fn speak(agent_id: Uuid, tick_id: i64, content: String) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::SPEAK,
            Some(serde_json::json!({ "content": content })),
        )
    }

    /// 创建 give 意图
    pub fn give(
        agent_id: Uuid,
        tick_id: i64,
        target_id: Uuid,
        item_id: &str,
        quantity: i32,
    ) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::GIVE,
            Some(serde_json::json!({
                "target_agent_id": target_id.to_string(),
                "item_id": item_id,
                "quantity": quantity
            })),
        )
    }

    /// 创建 steal 意图
    pub fn steal(agent_id: Uuid, tick_id: i64, target_id: Uuid, item_id: &str) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::STEAL,
            Some(serde_json::json!({
                "target_agent_id": target_id.to_string(),
                "item_id": item_id
            })),
        )
    }

    /// 创建 move 意图
    pub fn move_to(agent_id: Uuid, tick_id: i64, target_location: &str) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::MOVE,
            Some(serde_json::json!({
                "target_location": target_location
            })),
        )
    }

    /// 创建 pickup 意图
    pub fn pickup(agent_id: Uuid, tick_id: i64, item_id: &str) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::PICKUP,
            Some(serde_json::json!({ "item_id": item_id })),
        )
    }

    /// 创建 use 意图
    pub fn use_item(agent_id: Uuid, tick_id: i64, item_id: &str) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::USE,
            Some(serde_json::json!({ "item_id": item_id })),
        )
    }

    /// 创建 drop 意图
    pub fn drop_item(agent_id: Uuid, tick_id: i64, item_id: &str, quantity: i32) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::DROP,
            Some(serde_json::json!({ "item_id": item_id, "quantity": quantity })),
        )
    }

    /// 创建 gather 意图
    pub fn gather(agent_id: Uuid, tick_id: i64, target_id: &str) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::GATHER,
            Some(serde_json::json!({ "target_id": target_id })),
        )
    }

    /// 创建 craft 意图
    pub fn craft(agent_id: Uuid, tick_id: i64, recipe_id: &str) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::CRAFT,
            Some(serde_json::json!({ "recipe_id": recipe_id })),
        )
    }

    /// 创建 attack 意图
    pub fn attack(agent_id: Uuid, tick_id: i64, target_id: Uuid) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::ATTACK,
            Some(serde_json::json!({
                "target_agent_id": target_id.to_string()
            })),
        )
    }

    /// 创建 trade 意图
    pub fn trade(agent_id: Uuid, tick_id: i64, target_id: Uuid, item_id: &str, price: i32) -> Self {
        Self::new(
            agent_id,
            tick_id,
            ActionType::TRADE,
            Some(serde_json::json!({
                "target_agent_id": target_id.to_string(),
                "item_id": item_id,
                "price": price
            })),
        )
    }

    /// 设置思考日志
    pub fn with_thought(mut self, thought: String) -> Self {
        self.thought_log = Some(thought);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_type_new() {
        let action = ActionType::new("custom_action");
        assert_eq!(action.as_str(), "custom_action");
    }

    #[test]
    fn test_action_type_serde() {
        let action = ActionType::new("idle");
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"idle\"");

        let parsed: ActionType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_str(), "idle");
    }

    #[test]
    fn test_action_type_custom() {
        let action = ActionType::new("meditate");
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"meditate\"");

        let parsed: ActionType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_str(), "meditate");
    }

    #[test]
    fn test_intent_new() {
        let agent_id = Uuid::new_v4();
        let intent = Intent::new(
            agent_id,
            1,
            "custom_action",
            Some(serde_json::json!({ "param": "value" })),
        );
        assert_eq!(intent.action_type.as_str(), "custom_action");
        assert_eq!(intent.tick_id, 1);
    }

    #[test]
    fn test_intent_idle() {
        let agent_id = Uuid::new_v4();
        let intent = Intent::idle(agent_id, 1);
        assert_eq!(intent.action_type.as_str(), "idle");
        assert_eq!(intent.tick_id, 1);
    }

    #[test]
    fn test_intent_speak() {
        let agent_id = Uuid::new_v4();
        let intent = Intent::speak(agent_id, 2, "Hello".to_string());
        assert_eq!(intent.action_type.as_str(), "speak");
        assert!(intent.action_data.is_some());
    }

    #[test]
    fn test_intent_with_thought() {
        let agent_id = Uuid::new_v4();
        let intent = Intent::idle(agent_id, 1).with_thought("Thinking...".to_string());
        assert_eq!(intent.thought_log, Some("Thinking...".to_string()));
    }
}
