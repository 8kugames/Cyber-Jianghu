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
use super::rules::CalendarConfig;

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

/// 天数（1-30）转中文，10/20/30 用"十/二十/三十"而非 digit_to_chinese 逐位
pub fn day_to_chinese(day: i32) -> String {
    if day <= 0 {
        return "零".to_string();
    }
    match day {
        1..=9 => digit_to_chinese(day),
        10 => "十".to_string(),
        11..=19 => format!("十{}", digit_to_chinese(day - 10)),
        20 => "二十".to_string(),
        21..=29 => format!("二十{}", digit_to_chinese(day - 20)),
        30 => "三十".to_string(),
        _ => digit_to_chinese(day),
    }
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

/// 月份名称（1-12 对应元月...腊月）
///
/// WorldTime 和 game_day_to_chinese 共用同一份月份命名，
/// 避免日期与时间格式化出现"不同规格的轮子"。
pub fn month_name(month: i32) -> &'static str {
    match month {
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
        _ => "未知月",
    }
}

/// 游戏日（1-based）转中文日期字符串
///
/// 格式："四四年二月四日"（无"天道历"前缀，无时辰）
/// 算法：game_day 是单调递增的累计游戏日，按 calendar 配置换算为 year/month/day。
/// 与 `WorldTime::to_chinese()` 形成"日期 vs 日期时间"语义区分。
pub fn game_day_to_chinese(game_day: i64, calendar: &CalendarConfig) -> String {
    let days_per_season = calendar.days_per_season as i64;
    let seasons_per_year = calendar.seasons_per_year as i64;
    let days_per_year = days_per_season * seasons_per_year;

    if days_per_year <= 0 {
        return format!("第{}日", game_day);
    }

    let gd0 = game_day - 1;
    let year = 1 + (gd0 / days_per_year) as i32;
    let month = 1 + ((gd0 % days_per_year) / days_per_season) as i32;
    let day = 1 + (gd0 % days_per_season) as i32;

    format!(
        "{}年{}{}日",
        number_to_chinese(year),
        month_name(month),
        day_to_chinese(day)
    )
}

/// WorldTime → 游戏日（1-based）
///
/// `game_day_to_chinese` 的反向运算。服务端存储 WorldTime，agent 端查询
/// daily_summary/soul_cycle 时需要将 WorldTime 还原为 game_day 以命中 DB 索引。
/// 协议层提供统一实现，server 与 agent 共用。
pub fn game_day_from_world_time(wt: &WorldTime, calendar: &CalendarConfig) -> i64 {
    let days_per_season = calendar.days_per_season as i64;
    let seasons_per_year = calendar.seasons_per_year as i64;
    let days_per_year = days_per_season * seasons_per_year;

    if days_per_year <= 0 {
        return 0;
    }

    (wt.year as i64 - 1) * days_per_year + (wt.month as i64 - 1) * days_per_season + wt.day as i64
}

impl WorldTime {
    /// 格式化为中文风格时间表述
    ///
    /// 格式："三二五年元月四日申时"
    pub fn to_chinese(&self) -> String {
        let year = number_to_chinese(self.year);
        let month = month_name(self.month);
        let day = day_to_chinese(self.day);
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
    /// 观察行为
    Observation,
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
            WorldEventType::Observation => write!(f, "observation"),
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
            "observation" => Ok(WorldEventType::Observation),
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
            WorldEventType::Observation => "observation",
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
    /// 死因分类（satiation/hydration/hp/old_age/environmental）
    pub cause: String,
    /// 教训文本（供 LLM 参考的自然语言描述）
    pub lesson: String,
    /// 该死因的累计死亡人数
    pub death_count: i32,
    /// 该死因的平均存活 tick 数
    pub avg_survival_ticks: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_to_chinese_basic() {
        assert_eq!(day_to_chinese(1), "一");
        assert_eq!(day_to_chinese(4), "四");
        assert_eq!(day_to_chinese(9), "九");
    }

    #[test]
    fn day_to_chinese_ten_boundary() {
        assert_eq!(day_to_chinese(10), "十");
        assert_eq!(day_to_chinese(15), "十五");
        assert_eq!(day_to_chinese(19), "十九");
    }

    #[test]
    fn day_to_chinese_twenty_thirty() {
        assert_eq!(day_to_chinese(20), "二十");
        assert_eq!(day_to_chinese(25), "二十五");
        assert_eq!(day_to_chinese(30), "三十");
    }

    #[test]
    fn world_time_to_chinese_day_10() {
        let wt = WorldTime {
            year: 1,
            month: 1,
            day: 10,
            hour: 0,
            minute: 0,
            second: 0,
            weather: "晴".to_string(),
        };
        assert_eq!(wt.to_chinese(), "一年元月十日子时");
    }

    #[test]
    fn world_time_to_chinese_day_4() {
        let wt = WorldTime {
            year: 274,
            month: 1,
            day: 4,
            hour: 1,
            minute: 0,
            second: 0,
            weather: "晴".to_string(),
        };
        assert_eq!(wt.to_chinese(), "二七四年元月四日子时");
    }

    #[test]
    fn world_time_to_chinese_day_30() {
        let wt = WorldTime {
            year: 1,
            month: 1,
            day: 30,
            hour: 16,
            minute: 0,
            second: 0,
            weather: "晴".to_string(),
        };
        assert_eq!(wt.to_chinese(), "一年元月三十日申时");
    }

    fn default_calendar() -> CalendarConfig {
        CalendarConfig {
            days_per_season: 10,
            seasons_per_year: 4,
        }
    }

    #[test]
    fn game_day_to_chinese_day_1() {
        let cal = default_calendar();
        assert_eq!(game_day_to_chinese(1, &cal), "一年元月一日");
    }

    #[test]
    fn game_day_to_chinese_day_10() {
        let cal = default_calendar();
        assert_eq!(game_day_to_chinese(10, &cal), "一年元月十日");
    }

    #[test]
    fn game_day_to_chinese_day_11() {
        let cal = default_calendar();
        assert_eq!(game_day_to_chinese(11, &cal), "一年二月一日");
    }

    #[test]
    fn game_day_to_chinese_day_40() {
        let cal = default_calendar();
        assert_eq!(game_day_to_chinese(40, &cal), "一年四月十日");
    }

    #[test]
    fn game_day_to_chinese_day_41() {
        let cal = default_calendar();
        assert_eq!(game_day_to_chinese(41, &cal), "二年元月一日");
    }

    #[test]
    fn game_day_to_chinese_year_44() {
        let cal = default_calendar();
        assert_eq!(game_day_to_chinese(1734, &cal), "四四年二月四日");
    }

    #[test]
    fn game_day_to_chinese_custom_calendar() {
        let cal = CalendarConfig {
            days_per_season: 15,
            seasons_per_year: 3,
        };
        assert_eq!(game_day_to_chinese(1, &cal), "一年元月一日");
        assert_eq!(game_day_to_chinese(16, &cal), "一年二月一日");
        assert_eq!(game_day_to_chinese(46, &cal), "二年元月一日");
    }

    #[test]
    fn game_day_to_chinese_invalid_calendar() {
        let cal = CalendarConfig {
            days_per_season: 0,
            seasons_per_year: 0,
        };
        assert_eq!(game_day_to_chinese(5, &cal), "第5日");
    }

    fn wt(year: i32, month: i32, day: i32) -> WorldTime {
        WorldTime {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
            weather: String::new(),
        }
    }

    #[test]
    fn game_day_from_world_time_roundtrip_day_1() {
        let cal = default_calendar();
        assert_eq!(game_day_from_world_time(&wt(1, 1, 1), &cal), 1);
    }

    #[test]
    fn game_day_from_world_time_roundtrip_year_boundary() {
        let cal = default_calendar();
        assert_eq!(game_day_from_world_time(&wt(1, 4, 10), &cal), 40);
        assert_eq!(game_day_from_world_time(&wt(2, 1, 1), &cal), 41);
    }

    #[test]
    fn game_day_from_world_time_custom_calendar() {
        let cal = CalendarConfig {
            days_per_season: 15,
            seasons_per_year: 3,
        };
        assert_eq!(game_day_from_world_time(&wt(1, 1, 1), &cal), 1);
        assert_eq!(game_day_from_world_time(&wt(1, 2, 1), &cal), 16);
        assert_eq!(game_day_from_world_time(&wt(2, 1, 1), &cal), 46);
    }

    #[test]
    fn game_day_from_world_time_invalid_calendar() {
        let cal = CalendarConfig {
            days_per_season: 0,
            seasons_per_year: 0,
        };
        assert_eq!(game_day_from_world_time(&wt(1, 1, 1), &cal), 0);
    }

    /// 协议层不强制输入合法性——这些值在业务层由 WorldTime 上界守护。
    /// 测试目的是记录当前数学行为，防止无意中变更。
    #[test]
    fn game_day_from_world_time_out_of_range_inputs() {
        let cal = default_calendar();
        assert_eq!(game_day_from_world_time(&wt(0, 0, 0), &cal), -50);
        assert_eq!(game_day_from_world_time(&wt(1, 5, 11), &cal), 51);
        assert_eq!(game_day_from_world_time(&wt(-1, 1, 1), &cal), -79);
    }

    #[test]
    fn month_name_full_coverage() {
        assert_eq!(month_name(1), "元月");
        assert_eq!(month_name(12), "腊月");
        assert_eq!(month_name(0), "未知月");
        assert_eq!(month_name(13), "未知月");
    }
}
