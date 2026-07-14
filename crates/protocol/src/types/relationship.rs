//! 关系协议契约类型
//!
//! 为 C 阶段（数据可达）的关系存储与同步预留契约。
//! A 阶段只定义协议类型，不建服务端存储、不建同步链路。
//!
//! 字段集严格对齐 agent 端 `crates/agent/src/component/social/relationship_types.rs`。
//! 时间戳用 i64 Unix 毫秒（与 protocol 的 Pong.timestamp / AgentDied.died_at 对齐），
//! 不引入 DateTime<Utc> 到 protocol 层。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 关系关键事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipKeyEvent {
    /// Tick ID
    pub tick_id: i64,
    /// 事件类型（如：对话、交易、攻击、帮助）
    pub event_type: String,
    /// 事件描述
    pub description: String,
    /// 好感度变化
    pub favorability_delta: i32,
    /// 事件时间戳（Unix 毫秒）
    pub timestamp: i64,
}

/// 关系记忆（Agent A 对 Agent B 的单向关系）
///
/// 存储对某个目标 Agent 的关系记忆。A 阶段定义契约，
/// C 阶段建服务端存储 + agent→server 同步链路。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipMemory {
    /// 目标 Agent ID
    pub target_agent_id: Uuid,
    /// 目标 Agent 名称
    pub target_name: String,
    /// 好感度（-100 到 100，0 为中性）
    pub favorability: i32,
    /// 关键事件列表（FIFO，最多 20 条）
    pub key_events: Vec<RelationshipKeyEvent>,
    /// 最后交互的 Tick ID
    pub last_interaction_tick: i64,
    /// 最后更新时间（Unix 毫秒）
    pub updated_at: i64,
    /// AI 自主生成的好感度叙事化描述（20字以内）
    pub self_description: String,
    /// 描述生成时的 Tick ID
    pub description_tick: i64,
}
