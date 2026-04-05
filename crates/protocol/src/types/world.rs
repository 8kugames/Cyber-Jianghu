//! 世界状态相关类型
//!
//! 包含世界时间、事件和完整世界状态

use serde::{Deserialize, Serialize};
use std::fmt;

use std::str::FromStr;
use thiserror::Error;
use uuid::Uuid;

use super::{entities::*, locations::Location};

/// 世界时间
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldTime {
    /// 年份
    pub year: i32,

    /// 月份（1-12）
    pub month: i32,

    /// 日期（1-30）
    pub day: i32,

    /// 小时（0-23）
    pub hour: i32,

    /// 分钟（0-59）
    pub minute: i32,

    /// 秒（0-59）
    pub second: i32,

    /// 天气（MVP 阶段固定为"晴"）
    pub weather: String,
}

/// 世界事件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldEventType {
    /// 公开说话（speak）
    PublicMessage,
    /// 密语通知（whisper 摘要，不含内容）
    PrivateDialogue,
    /// 动作结果
    ActionResult,
    /// 环境变化
    EnvironmentalChange,
    /// 状态变更（如死亡、复活）
    StateChange,
    /// 时间更新
    TimeUpdate,
    /// 系统通知
    SystemNotification,
    /// 死亡通知
    DeathNotification,
    /// 社交互动
    SocialInteraction,
}

#[derive(Debug, Clone, Error)]
pub struct ParseWorldEventTypeError(String);

impl fmt::Display for ParseWorldEventTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Invalid WorldEventType: {}", self.0)
    }
}

impl fmt::Display for WorldEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorldEventType::PublicMessage => write!(f, "public_message"),
            WorldEventType::PrivateDialogue => write!(f, "private_dialogue"),
            WorldEventType::ActionResult => write!(f, "action_result"),
            WorldEventType::EnvironmentalChange => write!(f, "environmental_change"),
            WorldEventType::StateChange => write!(f, "state_change"),
            WorldEventType::TimeUpdate => write!(f, "time_update"),
            WorldEventType::SystemNotification => write!(f, "system_notification"),
            WorldEventType::DeathNotification => write!(f, "death_notification"),
            WorldEventType::SocialInteraction => write!(f, "social_interaction"),
        }
    }
}

impl FromStr for WorldEventType {
    type Err = ParseWorldEventTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "public_message" => Ok(WorldEventType::PublicMessage),
            "private_dialogue" => Ok(WorldEventType::PrivateDialogue),
            "action_result" => Ok(WorldEventType::ActionResult),
            "environmental_change" => Ok(WorldEventType::EnvironmentalChange),
            "state_change" => Ok(WorldEventType::StateChange),
            "time_update" => Ok(WorldEventType::TimeUpdate),
            "system_notification" => Ok(WorldEventType::SystemNotification),
            "death_notification" => Ok(WorldEventType::DeathNotification),
            "social_interaction" => Ok(WorldEventType::SocialInteraction),
            _ => Err(ParseWorldEventTypeError(s.to_string())),
        }
    }
}

impl WorldEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorldEventType::PublicMessage => "public_message",
            WorldEventType::PrivateDialogue => "private_dialogue",
            WorldEventType::ActionResult => "action_result",
            WorldEventType::EnvironmentalChange => "environmental_change",
            WorldEventType::StateChange => "state_change",
            WorldEventType::TimeUpdate => "time_update",
            WorldEventType::SystemNotification => "system_notification",
            WorldEventType::DeathNotification => "death_notification",
            WorldEventType::SocialInteraction => "social_interaction",
        }
    }
}

/// 世界事件
///
/// 结构化事件数据，用于从服务端传递给客户端记忆系统
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldEvent {
    /// 事件类型
    #[serde(flatten)]
    pub event_type: WorldEventType,

    /// Tick 编号
    pub tick_id: i64,

    /// 事件描述（自然语言）
    pub description: String,

    /// 元数据（JSON 格式，包含参与实体、物品、地点等）
    pub metadata: serde_json::Value,
}


/// 密语记录（不含内容，仅索引）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateDialogueRecord {
    /// 会话 ID
    pub session_id: String,
    /// 参与者 A ID
    pub agent_a_id: Uuid,
    /// 参与者 A 名称
    pub agent_a_name: String,
    /// 参与者 B ID
    pub agent_b_id: Uuid,
    /// 参与者 B 名称
    pub agent_b_name: String,
    /// 消息条数
    pub message_count: u32,
    /// 最后一条消息的发送者名称
    pub last_message_from: String,
}

/// 世界状态
///
/// 每个 Tick 开始时，服务端通过 WebSocket 下发给所有 Agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldState {
    /// 事件类型（固定为 "world_state"）
    pub event_type: String,

    /// Tick 编号
    pub tick_id: i64,

    /// 当前 Agent ID（可选，用于客户端识别）
    #[serde(default)]
    pub agent_id: Option<Uuid>,

    /// 世界时间
    pub world_time: WorldTime,

    /// 当前节点信息
    pub location: Location,

    /// Agent 自身状态
    pub self_state: AgentSelfState,

    /// 周围实体（其他 Agent）
    #[serde(default)]
    pub entities: Vec<Entity>,

    /// 场景中的可拾取物品
    #[serde(default)]
    pub nearby_items: Vec<SceneItem>,

    /// 事件日志（最近发生的事件，结构化格式）
    #[serde(default)]
    pub events_log: Vec<WorldEvent>,

    /// 上一 Tick 的密语索引（不含内容，仅供 LLM 知道谁和谁说了话）
    #[serde(default)]
    pub private_dialogue_log: Vec<PrivateDialogueRecord>,

    /// 关单时刻的 Unix 毫秒时间戳（绝对时间）
    /// Agent 应在此时刻之前提交意图
    #[serde(default)]
    pub deadline_ms: u64,
}
