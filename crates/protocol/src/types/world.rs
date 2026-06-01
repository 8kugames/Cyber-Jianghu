//! 世界状态相关类型
//!
//! 包含世界时间、事件和完整世界状态

use serde::{Deserialize, Serialize};
use std::fmt;

use std::str::FromStr;
use thiserror::Error;
use uuid::Uuid;

use super::{entities::*, locations::Location};

use super::actions::ExecutionSummary;

/// 世界时间
///
/// 实际范围由 server time.yaml 控制（默认 seasons_per_year=4, days_per_season=10）：
/// - month: 1..seasons_per_year（对应季节）
/// - day: 1..days_per_season
/// - hour: 0..hours_per_day-1
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldTime {
    /// 年份
    pub year: i32,

    /// 月份 / 季节序号（范围由 seasons_per_year 决定，默认 1-4）
    pub month: i32,

    /// 日期（范围由 days_per_season 决定，默认 1-10）
    pub day: i32,

    /// 小时（范围由 hours_per_day 决定，默认 0-11）
    pub hour: i32,

    /// 分钟（0-59）
    pub minute: i32,

    /// 秒（0-59）
    pub second: i32,

    /// 天气
    pub weather: String,
}

/// 数字转中文大写（0-9）
pub fn digit_to_chinese(n: i32) -> String {
    let digits = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
    n.to_string()
        .chars()
        .filter_map(|c| c.to_digit(10).and_then(|d| digits.get(d as usize)).copied())
        .collect()
}

/// 数字转中文大写
pub fn number_to_chinese(n: i32) -> String {
    if n == 0 {
        return "零".to_string();
    }
    digit_to_chinese(n)
}

/// 时辰名称（十二时辰制，每时辰两小时）
pub fn shichen_name(hour: i32) -> &'static str {
    match hour {
        0..=1 => "子时",
        2..=3 => "丑时",
        4..=5 => "寅时",
        6..=7 => "卯时",
        8..=9 => "辰时",
        10..=11 => "巳时",
        12..=13 => "午时",
        14..=15 => "未时",
        16..=17 => "申时",
        18..=19 => "酉时",
        20..=21 => "戌时",
        22..=23 => "亥时",
        _ => "时辰",
    }
}

impl WorldTime {
    /// 格式化为中文风格时间表述
    ///
    /// 格式："天道历三二五年元月四日申时"
    pub fn to_chinese(&self) -> String {
        let year = number_to_chinese(self.year);
        let month = match self.month {
            1 => "元月",
            2 => "二月",
            3 => "三月",
            4 => "四月",
            5 => "五月",
            6 => "六月",
            7 => "七月",
            8 => "八月",
            9 => "九月",
            10 => "十月",
            11 => "十一月",
            12 => "腊月",
            _ => return format!("第{}天{:02}:{:02}", self.day, self.hour, self.minute),
        };
        let day = number_to_chinese(self.day);
        let shichen = shichen_name(self.hour);
        format!("{}年{}{}日{}", year, month, day, shichen)
    }
}

/// 世界事件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

    /// 上一次 Pipeline 执行汇总（无数值泄露风险）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_execution_summary: Option<ExecutionSummary>,

    /// 跨 Agent 传承教训（按死因聚合的集体经验）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lessons_learned: Vec<PublicLesson>,
}

/// 跨 Agent 传承教训条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicLesson {
    /// 死因分类（hunger/thirst/hp/old_age/environmental）
    pub cause: String,
    /// 教训文本（供 LLM 参考的自然语言描述）
    pub lesson: String,
    /// 该死因的累计死亡人数
    pub death_count: i32,
    /// 该死因的平均存活 tick 数
    pub avg_survival_ticks: i64,
}
