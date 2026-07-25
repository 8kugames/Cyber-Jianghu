//! 训练数据自动导出（Server 端 SFT Export）
//!
//! 设计文档: docs/superpowers/specs/2026-07-25-training-export-design.md
//! 定时后台任务 + 手动 POST 触发, 产出 vLLM/Axolotl 兼容的 SFT JSONL.
//! 绝不影响 24h 在线的热路径 (见 spec §2 干扰面矩阵).

pub mod checkpoint;
pub mod config;
pub mod handlers;
pub mod runner;
pub mod scheduler;
pub mod sft_transform;

use serde::{Deserialize, Serialize};

/// 触发源
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    /// 定时后台
    Scheduled,
    /// POST 触发
    Manual,
}

/// Run 状态
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    /// 并发跳过/超限跳过
    Skipped,
}

/// Run 元数据 (写 run=<id>.meta.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMetadata {
    /// ULID, 时序有序
    pub run_id: String,
    pub status: RunStatus,
    pub triggered_by: TriggerSource,
    /// Unix ms
    pub started_at: i64,
    pub completed_at: Option<i64>,
    /// None = 所有 agent
    pub agent_id_filter: Option<uuid::Uuid>,
    pub force_full: bool,
    pub trace_count: usize,
    pub sample_count: usize,
    /// 相对 data_dir
    pub output_path: String,
    pub output_size_bytes: u64,
    pub error: Option<String>,
    /// 元数据格式版本, 未来迁移用
    pub schema_version: u32,
}

impl RunMetadata {
    /// 创建一个 pending 状态的新 run
    pub fn new_pending(run_id: String, triggered_by: TriggerSource) -> Self {
        Self {
            run_id,
            status: RunStatus::Pending,
            triggered_by,
            started_at: chrono::Utc::now().timestamp_millis(),
            completed_at: None,
            agent_id_filter: None,
            force_full: false,
            trace_count: 0,
            sample_count: 0,
            output_path: String::new(),
            output_size_bytes: 0,
            error: None,
            schema_version: 1,
        }
    }
}
