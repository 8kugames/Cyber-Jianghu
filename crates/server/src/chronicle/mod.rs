// ============================================================================
// 群像传记模块
// ============================================================================
//
// 每 7 游戏日生成一份《群像传记》，记录世界周期的故事
//
// 模块结构：
// - collector: 数据采集，从数据库聚合 7 日数据
// - generator: 生成器，模板 + LLM 两种模式
// - storage: 存储，持久化到 chronicles 表
//
// 生成流程：
// 1. collector 采集原始数据
// 2. generator 生成模板版本
// 3. generator 生成 LLM 版本（可选）
// 4. storage 持久化
// ============================================================================

pub mod collector;
pub mod generator;
pub mod storage;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// 群像传记数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chronicle {
    pub id: i64,
    pub chronicle_id: String,
    pub period_start: i64,
    pub period_end: i64,
    pub game_day_start: i32,
    pub game_day_end: i32,
    pub season: String,
    pub summary: String,
    pub summary_llm: Option<String>,
    pub agent_count: i32,
    pub actions_count: i32,
    pub highlights: Vec<Highlight>,
    pub agent_summaries: Vec<AgentSummary>,
    pub action_stats: ActionStats,
    pub location_stats: Vec<LocationStat>,
    pub deaths: i32,
    pub births: i32,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 关键事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Highlight {
    pub tick_id: i64,
    pub event_type: String,
    pub description: String,
    pub agent_id: Option<uuid::Uuid>,
    pub agent_name: Option<String>,
}

/// Agent 简报
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub agent_id: uuid::Uuid,
    pub name: String,
    pub location: String,
    pub actions_count: i32,
    pub top_actions: Vec<String>,
    pub narrative: Option<String>,
    pub died_this_period: bool,
}

/// 动作统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionStats {
    pub total: i32,
    pub by_type: std::collections::HashMap<String, i32>,
    pub success_rate: f64,
}

/// 地点统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationStat {
    pub location: String,
    pub count: i32,
    pub percentage: f64,
}

// Re-export for handlers
pub use storage::ChronicleMeta;

/// 生成配置
#[derive(Debug, Clone, Deserialize)]
pub struct ChronicleConfig {
    /// 每个周期的 tick 数（从 time.yaml 读取）
    pub period_ticks: i64,
    /// 关键事件每个类型的最大数量
    pub highlight_threshold: i32,
}

impl Default for ChronicleConfig {
    fn default() -> Self {
        Self {
            // 从 time.yaml 动态读取：days_per_season * hours_per_day * ticks_per_hour
            // 假设 7 游戏日为一个周期
            period_ticks: Self::calculate_period_ticks(),
            highlight_threshold: 3,
        }
    }
}

impl ChronicleConfig {
    /// 从 time.yaml 配置计算周期 tick 数
    fn calculate_period_ticks() -> i64 {
        crate::game_data::registry::TimeRegistry::get_config()
            .map(|c| {
                // 7 游戏日 * 每天 24 小时 * 每小时 tick 数
                // 注：days_per_season 用于季节周期，chronicle 固定为 7 日
                let days_per_period = 7;
                (days_per_period * c.hours_per_day * c.ticks_per_hour) as i64
            })
            .unwrap_or(168) // 回退值
    }

    /// 获取周期天数配置（供外部使用）
    pub fn period_days() -> i64 {
        7 // 固定 7 日
    }
}

/// 生成并存储一份群像传记
pub async fn generate_and_store(
    period_start: i64,
    period_end: i64,
    db_pool: &crate::db::DbPool,
) -> Result<Chronicle> {
    // 1. 采集数据
    let data = collector::collect(db_pool, period_start, period_end).await?;

    // 2. 模板生成
    let summary = generator::generate_template(&data)?;

    // 3. 持久化
    let chronicle = storage::store(db_pool, &data, &summary).await?;

    // 4. LLM 生成（异步，不阻塞）
    // generate_llm 内部会根据 llm.yaml 配置决定是否生成
    let chronicle_id = chronicle.chronicle_id.clone();
    let data_clone = data.clone();
    let db_pool_clone = db_pool.clone();
    tokio::spawn(async move {
        match generator::generate_llm(&data_clone).await {
            Ok(summary_llm) => {
                if let Err(e) =
                    storage::update_llm_summary(&db_pool_clone, &chronicle_id, &summary_llm).await
                {
                    tracing::warn!("LLM 摘要更新失败: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!("LLM 生成失败: {}", e);
            }
        }
    });

    Ok(chronicle)
}

/// 计算周期的起始 tick_id
pub fn calculate_period_start(tick_id: i64) -> i64 {
    let period_ticks = ChronicleConfig::default().period_ticks;
    ((tick_id.saturating_sub(1)) / period_ticks) * period_ticks + 1
}
