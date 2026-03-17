// ============================================================================
// Agent 相关数据结构
// ============================================================================

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// Agent基本信息
///
/// 存储Agent的基本信息，包括名称、人设Prompt、认证token等
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Agent {
    /// Agent唯一ID（UUID）
    pub agent_id: Uuid,

    /// Agent名称（如：老板娘、富商、刀客、新秀、小偷）
    pub name: String,

    /// Agent人设Prompt（LLM使用）
    /// 定义Agent的性格、行为规则等
    pub system_prompt: String,

    /// 认证token（WebSocket连接时使用）
    pub auth_token: String,

    /// 创建时间
    pub created_at: DateTime<Utc>,

    /// 最后一次上报意图的时间
    pub last_tick_online: Option<DateTime<Utc>>,
}

/// Agent状态
///
/// 每Tick记录一次Agent的状态快照
/// 使用 COI 架构：组件组合代替 HashMap 扁平结构
/// - primary_attributes: 先天属性组件（力量、敏捷、体质等）
/// - status: 状态值组件（HP、体力、饥饿、口渴等）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// 状态记录ID
    pub id: i64,

    /// Agent ID
    pub agent_id: Uuid,

    /// Tick编号（递增）
    pub tick_id: i64,

    /// 先天属性组件（力量、敏捷、体质、智力、魅力、福缘）
    pub primary_attributes: crate::game_data::types::AttributeComponent,

    /// 状态值组件（HP、体力、饥饿、口渴、内力、理智、声望、银两）
    pub status: crate::game_data::types::StatusComponent,

    /// 当前所在节点ID
    pub node_id: String,

    /// 是否存活
    pub is_alive: bool,

    /// 本Tick内是否已清空过背包（防止重复清空）
    pub inventory_cleared_this_tick: bool,

    /// 状态记录时间
    pub created_at: DateTime<Utc>,
}
