//! 世界状态相关类型
//!
//! 包含世界时间、事件和完整世界状态

use serde::{Deserialize, Serialize};
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

/// 世界事件
///
/// 结构化事件数据，用于从服务端传递给客户端记忆系统
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldEvent {
    /// 事件类型（action_result, environmental_change, social_interaction）
    pub event_type: String,

    /// Tick 编号
    pub tick_id: i64,

    /// 事件描述（自然语言）
    pub description: String,

    /// 元数据（JSON 格式，包含参与实体、物品、地点等）
    pub metadata: serde_json::Value,
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

    /// 可用动作列表
    #[serde(default)]
    pub available_actions: Vec<AvailableAction>,
}
