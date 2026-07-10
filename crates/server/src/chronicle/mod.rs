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
// 2. 优先 LLM 生成（同步），失败则降级模板
// 3. 立即持久化主版本
// 4. 异步补充生成另一个版本
// 5. 进度可通过 /api/dashboard/chronicles/pending 追踪
// ============================================================================

use std::sync::Arc;
use tokio::sync::RwLock;

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
    /// 涌现事件（因果验证通过的事件链，持久化于 raw_data JSONB）
    #[serde(default)]
    pub emergence_events: Vec<crate::emergence::EmergenceEvent>,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// 服务端格式化的游戏内日期字符串（由 get_chronicle 填充）
    #[serde(default)]
    pub formatted_start_date: String,
    /// 服务端格式化的游戏内日期字符串（由 get_chronicle 填充）
    #[serde(default)]
    pub formatted_end_date: String,
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
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChronicleConfig {
    /// 每个周期的 tick 数（从 time.yaml 读取）
    pub period_ticks: i64,
    /// 关键事件每个类型的最大数量
    pub highlight_threshold: i32,
}

impl Default for ChronicleConfig {
    fn default() -> Self {
        Self {
            period_ticks: Self::calculate_period_ticks(),
            highlight_threshold: Self::get_highlight_threshold(),
        }
    }
}

impl ChronicleConfig {
    /// 从 time.yaml + game_rules.yaml 计算周期 tick 数
    /// tick_id 是真实秒数，需乘以 real_seconds_per_tick 转换
    fn calculate_period_ticks() -> i64 {
        let time_config = crate::game_data::registry::TimeRegistry::get_config();
        let chronicle_config = crate::game_data::registry::ChronicleRegistry::get_config();
        let registry = crate::game_data::registry_or_error()
            .inspect_err(|e| {
                tracing::warn!("calculate_period_ticks: registry 不可用，使用默认值: {}", e)
            })
            .ok();

        // game_rules.yaml agent_state.tick.real_seconds_per_tick 默认值 60
        let real_seconds_per_tick = registry
            .map(|r| {
                r.get()
                    .game_rules
                    .data
                    .agent_state
                    .tick
                    .real_seconds_per_tick as i64
            })
            .unwrap_or(60);

        match (time_config, chronicle_config) {
            (Some(c), Some(chronicle)) => {
                (chronicle.days_per_period * c.hours_per_day * c.ticks_per_hour) as i64
                    * real_seconds_per_tick
            }
            (Some(c), None) => {
                (7 * c.hours_per_day * c.ticks_per_hour) as i64 * real_seconds_per_tick
            }
            _ => 5040, // 7 * 12 * 1 * 60 — 与默认配置一致
        }
    }

    /// 从 game_rules.yaml 获取 highlight_threshold
    fn get_highlight_threshold() -> i32 {
        crate::game_data::registry::ChronicleRegistry::get_config()
            .map(|c| c.highlight_threshold)
            .unwrap_or(3)
    }

    /// 获取周期天数配置（供外部使用）
    pub fn period_days() -> i64 {
        crate::game_data::registry::ChronicleRegistry::get_config()
            .map(|c| c.days_per_period as i64)
            .unwrap_or(7)
    }
}

/// 异步生成状态
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GenerationStatus {
    /// 等待中
    Pending,
    /// 正在生成
    Generating,
    /// 完成
    Completed,
    /// 失败
    Failed(String),
}

/// 异步生成任务
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GenerationTask {
    /// Chronicle ID
    pub chronicle_id: String,
    /// 任务状态
    pub status: GenerationStatus,
    /// 主版本类型
    pub primary_version: String, // "llm" 或 "template"
    /// 补充版本状态
    pub supplement_status: GenerationStatus,
    /// 任务开始时间
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// 完成时间
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// 全局异步任务跟踪器
pub struct GenerationTracker {
    tasks: RwLock<Vec<GenerationTask>>,
}

impl GenerationTracker {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(Vec::new()),
        }
    }

    /// 添加新任务
    pub async fn add_task(&self, chronicle_id: &str, primary_version: &str) -> String {
        let task_id = chronicle_id.to_string();
        let task = GenerationTask {
            chronicle_id: task_id.clone(),
            status: GenerationStatus::Generating,
            primary_version: primary_version.to_string(),
            supplement_status: GenerationStatus::Pending,
            started_at: chrono::Utc::now(),
            completed_at: None,
        };
        self.tasks.write().await.push(task);
        task_id
    }

    /// 更新补充版本状态
    pub async fn update_supplement(&self, chronicle_id: &str, status: GenerationStatus) {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.chronicle_id == chronicle_id) {
            task.supplement_status = status.clone();
            if matches!(status, GenerationStatus::Completed) {
                task.completed_at = Some(chrono::Utc::now());
                task.status = GenerationStatus::Completed;
            } else if let GenerationStatus::Failed(_) = status {
                task.completed_at = Some(chrono::Utc::now());
            }
        }
    }

    /// 获取所有任务
    pub async fn get_tasks(&self) -> Vec<GenerationTask> {
        self.tasks.read().await.clone()
    }

    /// 获取进行中的任务
    pub async fn get_pending_tasks(&self) -> Vec<GenerationTask> {
        self.tasks
            .read()
            .await
            .iter()
            .filter(|t| {
                matches!(
                    t.status,
                    GenerationStatus::Pending | GenerationStatus::Generating
                )
            })
            .cloned()
            .collect()
    }

    /// 清理已完成超过 1 小时的任务
    pub async fn cleanup(&self) {
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(1);
        let mut tasks = self.tasks.write().await;
        tasks.retain(|t| t.completed_at.map(|c| c > cutoff).unwrap_or(true));
    }
}

impl Default for GenerationTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局任务跟踪器实例
static GENERATION_TRACKER: std::sync::OnceLock<Arc<GenerationTracker>> = std::sync::OnceLock::new();

/// 获取全局任务跟踪器
pub fn get_generation_tracker() -> Arc<GenerationTracker> {
    GENERATION_TRACKER
        .get_or_init(|| Arc::new(GenerationTracker::new()))
        .clone()
}

/// 生成并存储一份群像传记
///
/// 策略：
/// 1. 优先尝试 LLM 生成（同步）
/// 2. LLM 失败时降级到模板版本
/// 3. 无论哪个成功，都立即存储（同步）
/// 4. 异步补充生成另一个版本并更新数据库
/// 5. 进度可通过 get_generation_tracker().get_pending_tasks() 追踪
pub async fn generate_and_store(
    period_start: i64,
    period_end: i64,
    db_pool: &crate::db::DbPool,
) -> Result<Chronicle> {
    // 1. 采集数据
    let data = collector::collect(db_pool, period_start, period_end).await?;
    tracing::info!(
        "群像传记数据采集完成: {} agents, {} highlights",
        data.agents.len(),
        data.highlights.len()
    );

    // 2. 生成策略：LLM 优先，模板兜底，两个版本都生成
    // - LLM 成功：summary_llm = LLM, summary = 模板（同步），异步生成 LLM 补充
    // - LLM 失败：summary = 模板（同步），summary_llm = None，异步生成 LLM 补充
    // 2.1 先同步生成模板（总是需要）
    // 空周期时使用配置的 empty_period_template
    let summary = if data.agents.is_empty() && data.highlights.is_empty() {
        crate::game_data::registry::ChronicleRegistry::get_config()
            .map(|c| c.empty_period_template)
            .unwrap_or_else(|| "此间风平浪静，江湖无事。".to_string())
    } else {
        generator::generate_template(&data)?
    };

    // 2.2 尝试 LLM（如果成功则作为补充版本异步存储）
    let summary_llm = match generator::generate_llm(&data).await {
        Ok(llm_summary) => {
            tracing::info!("LLM 生成成功");
            Some(llm_summary)
        }
        Err(e) => {
            tracing::warn!("LLM 生成失败: {}", e);
            None
        }
    };

    // 3. 立即持久化（两个版本都同步存储）
    let chronicle =
        storage::store_with_llm(db_pool, &data, &summary, summary_llm.as_deref()).await?;
    tracing::info!(
        "群像传记已存储: {} (summary: {}, summary_llm: {})",
        chronicle.chronicle_id,
        if summary.is_empty() { "空" } else { "有" },
        if summary_llm.is_some() { "有" } else { "空" }
    );

    // 4. 如果 LLM 失败，注册异步重试任务
    let tracker = get_generation_tracker();
    let chronicle_id = chronicle.chronicle_id.clone();

    if summary_llm.is_none() {
        // LLM 失败，异步重试生成 LLM 作为补充
        let task_id = tracker.add_task(&chronicle_id, "llm_retry").await;
        tracing::info!("[任务 {}] LLM 生成失败，注册异步重试任务", task_id);

        let chronicle_id_clone = chronicle_id.clone();
        let data_clone = data.clone();
        let db_pool_clone = db_pool.clone();
        let tracker_clone = tracker.clone();

        tokio::spawn(async move {
            tracing::info!("[任务 {}] 开始异步重试生成 LLM 版本", chronicle_id_clone);
            match generator::generate_llm(&data_clone).await {
                Ok(llm_summary) => {
                    tracing::info!("[任务 {}] LLM 重试成功，更新数据库", chronicle_id_clone);
                    if let Err(e) = storage::update_llm_summary(
                        &db_pool_clone,
                        &chronicle_id_clone,
                        &llm_summary,
                    )
                    .await
                    {
                        tracing::warn!("[任务 {}] LLM 摘要更新失败: {}", chronicle_id_clone, e);
                        tracker_clone
                            .update_supplement(
                                &chronicle_id_clone,
                                GenerationStatus::Failed(e.to_string()),
                            )
                            .await;
                    } else {
                        tracing::info!("[任务 {}] LLM 版本更新完成", chronicle_id_clone);
                        tracker_clone
                            .update_supplement(&chronicle_id_clone, GenerationStatus::Completed)
                            .await;
                    }
                }
                Err(e) => {
                    tracing::warn!("[任务 {}] 异步 LLM 重试失败: {}", chronicle_id_clone, e);
                    tracker_clone
                        .update_supplement(
                            &chronicle_id_clone,
                            GenerationStatus::Failed(e.to_string()),
                        )
                        .await;
                }
            }
        });
    } else {
        // LLM 成功，两个版本都有了
        let task_id = tracker.add_task(&chronicle_id, "both").await;
        tracing::info!("[任务 {}] LLM 和模板版本都已生成，无需异步任务", task_id);
        tracker
            .update_supplement(&chronicle_id, GenerationStatus::Completed)
            .await;
    }

    Ok(chronicle)
}

/// 计算周期的起始 tick_id
pub fn calculate_period_start(tick_id: i64) -> i64 {
    let period_ticks = ChronicleConfig::default().period_ticks;
    ((tick_id.saturating_sub(1)) / period_ticks) * period_ticks + 1
}

/// 截断文本（正确处理 UTF-8 字符边界）
pub(crate) fn truncate_text(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        return s.to_string();
    }
    // 确保截断点在字符边界上
    let end = s
        .char_indices()
        .nth(max_len.saturating_sub(3))
        .map(|(idx, _)| idx)
        .unwrap_or(s.len());
    format!("{}...", &s[..end])
}
