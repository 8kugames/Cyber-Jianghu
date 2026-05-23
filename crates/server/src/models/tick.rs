// ============================================================================
// Tick日志相关数据结构
// ============================================================================

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Tick执行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum TickStatus {
    /// 运行中
    Running,

    /// 已完成
    Completed,

    /// 失败
    Failed,
}

impl fmt::Display for TickStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl FromStr for TickStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Invalid tick status: {}", s)),
        }
    }
}

/// Tick日志
///
/// 记录每次Tick的执行信息，包括耗时、处理的Agent数量等
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TickLog {
    /// Tick编号
    pub tick_id: i64,

    /// 开始时间
    pub started_at: DateTime<Utc>,

    /// 完成时间
    pub completed_at: Option<DateTime<Utc>>,

    /// 执行耗时（毫秒）
    pub duration_ms: Option<i64>,

    /// 处理的Agent数量
    pub agents_processed: i32,

    /// 执行的动作数量
    pub actions_executed: i32,

    /// Tick状态
    pub status: TickStatus,

    /// 错误信息（如果失败）
    pub error_message: Option<String>,
}

impl TickLog {
    /// 创建新的Tick日志
    pub fn new(tick_id: i64) -> Self {
        Self {
            tick_id,
            started_at: Utc::now(),
            completed_at: None,
            duration_ms: None,
            agents_processed: 0,
            actions_executed: 0,
            status: TickStatus::Running,
            error_message: None,
        }
    }

    /// 标记Tick完成
    pub fn complete(&mut self, agents_processed: i32, actions_executed: i32) {
        self.completed_at = Some(Utc::now());
        self.duration_ms = Some(
            (self
                .completed_at
                .expect("tick must be completed before calculating duration")
                - self.started_at)
                .num_milliseconds(),
        );
        self.agents_processed = agents_processed;
        self.actions_executed = actions_executed;
        self.status = TickStatus::Completed;
    }

    /// 标记Tick失败（F-06）
    pub fn fail(&mut self, error_message: &str) {
        self.completed_at = Some(Utc::now());
        self.duration_ms = Some(
            (self
                .completed_at
                .expect("tick must be completed before calculating duration")
                - self.started_at)
                .num_milliseconds(),
        );
        self.status = TickStatus::Failed;
        self.error_message = Some(error_message.to_string());
    }
}
