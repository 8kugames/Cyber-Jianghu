// ============================================================================
// 意图和动作相关数据结构
// ============================================================================

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

use cyber_jianghu_protocol as protocol;

// Re-export ActionType from protocol
pub use protocol::ActionType;

/// Agent执行的动作
///
/// 记录Agent实际执行的动作及结果
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgentAction {
    /// 记录ID
    pub id: i64,

    /// Tick编号
    pub tick_id: i64,

    /// Agent ID
    pub agent_id: Uuid,

    /// 动作类型（原始值，如 idle, speak）
    pub action_type: ActionType,

    /// 动作中文描述（如 "休息，不做任何操作"）
    /// 从 actions.yaml 配置获取
    pub action_type_display: Option<String>,

    /// 动作参数
    pub action_data: Option<serde_json::Value>,

    /// 执行结果（success/failed）
    pub result: ActionResult,

    /// 执行结果详细描述（如 "休息后体力恢复了5点"）
    /// 从 ActionExecutionResult.message 获取
    pub result_message: Option<String>,

    /// ActorSoul 思考日志
    pub thought_log: Option<String>,

    /// ReflectorSoul 审查理由
    pub observer_thought: Option<String>,

    /// 叙事化经历描述
    pub narrative: Option<String>,

    /// 三魂循环元数据（JSONB）
    /// 由 agent 通过 WebSocket SoulCycleReport 消息上报
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soul_cycle_metadata: Option<serde_json::Value>,

    /// 记录时间
    pub created_at: DateTime<Utc>,
}

/// 动作执行结果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum ActionResult {
    /// 成功
    Success,

    /// 失败
    Failed,
}

impl fmt::Display for ActionResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl FromStr for ActionResult {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "success" => Ok(Self::Success),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Invalid action result: {}", s)),
        }
    }
}
