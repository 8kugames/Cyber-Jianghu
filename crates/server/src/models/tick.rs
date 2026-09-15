// ============================================================================
// Tick日志相关数据结构
// ============================================================================

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// TickStatus 与 protocol::sqlx_types 逐字同源，统一以 protocol 为唯一事实源
pub use cyber_jianghu_protocol::sqlx_types::TickStatus;

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

    /// 标记Tick失败
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
