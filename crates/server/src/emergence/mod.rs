// ============================================================================
// 涌现检测模块（Causal Emergence Detection）
// ============================================================================
//
// 机器可验证地回答项目核心假设："在生存压力下，AI 智能体会自发涌现
// 结盟、背叛、交易、厮杀等社会行为吗？"（白皮书 / MVP §6.1.3）
//
// 两阶段检测：
//   阶段 1 形态筛选：按 tick+node 聚类时空簇，阈值判定候选事件。
//   阶段 2 因果验证：验证 agent 间"感知→处理→定向回应"闭环，
//                    区分 causal_emergence（真因果）与 co_occurrence（仅共现/存疑）。
//
// 架构定位：观察者职责，只读查询（不修改世界状态），不侵入 tick 热路径。
// 确定性不变量：同输入 → 同输出（max-tick 幂等缓存）。
//
// 模块结构：
//   - config:   emergence.yaml 反序列化类型
//   - detector: 纯函数检测逻辑（零 DB，可单测）
//   - loader:   sqlx 查询（与判定逻辑分离）
// ============================================================================

pub mod config;
pub mod detector;
pub mod loader;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

pub use config::EmergenceConfig;
pub use detector::{ActionRow, ActionSummary, CausalEdge, EmergenceEvent};

use crate::game_data::loaders::config_format::load_config;

// ============================================================================
// 结果类型
// ============================================================================

/// 单次涌现检测的完整结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    pub tick_start: i64,
    pub tick_end: i64,
    pub candidate_count: usize,
    pub causal_emergence_count: usize,
    pub co_occurrence_count: usize,
    pub events: Vec<EmergenceEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthMetrics>,
}

/// MVP §6.1.1/§6.1.2 健康度
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HealthMetrics {
    // §6.1.1 运行稳定性
    pub ticks_total: i64,
    pub ticks_completed: i64,
    pub ticks_failed: i64,
    pub ticks_running: i64,
    pub tick_completion_rate: f64,
    pub continuous_run_seconds: f64,
    /// 超时率近似（= 1 − 已提交/应参与），非 MVP 字面30秒墙钟超时
    pub timeout_rate_approx: f64,
    pub agents_expected: i32,
    pub agents_submitted: i32,
    // §6.1.2 生存能力
    pub agents_alive: i32,
    pub min_survivors_required: i32,
    pub survivors_pass: bool,
    pub per_agent_supply: HashMap<Uuid, i32>,
    pub min_supply_required: i32,
    pub supply_pass: bool,
}

// ============================================================================
// max-tick 幂等缓存
// ============================================================================

/// 缓存键：(tick_start, tick_end)
type CacheKey = (i64, i64);

struct CachedResult {
    result: Arc<DetectionResult>,
    /// 计算时的 max tick；max tick 推进则缓存失效
    computed_at_tick: i64,
}

static CACHE: RwLock<Option<HashMap<CacheKey, CachedResult>>> = RwLock::const_new(None);

/// 清空缓存（配置热重载时调用）
pub async fn invalidate_cache() {
    *CACHE.write().await = None;
}

// ============================================================================
// 公开入口
// ============================================================================

/// 加载 emergence.yaml 配置（fail-fast：缺失报错）
pub fn load_emergence_config(config_dir: &Path) -> Result<EmergenceConfig> {
    let yaml_path = config_dir.join("emergence.yaml");
    load_config(&yaml_path).with_context(|| format!("加载涌现检测配置失败: {}", yaml_path.display()))
}

/// 涌现检测入口：给定窗口，跑两阶段检测。
///
/// `include_health=true` 时附带 MVP 健康度。
/// handler 和 chronicle 都走这里，逻辑只有一份。
/// max-tick 幂等缓存：同 (start, end) 在同一 max-tick 内直接复用。
pub async fn detect_window(
    db_pool: &crate::db::DbPool,
    config: &EmergenceConfig,
    tick_start: i64,
    tick_end: i64,
    include_health: bool,
) -> Result<DetectionResult> {
    // 缓存检查：同窗口同 max-tick 幂等。
    // 注意：若调用方请求 health 但缓存结果不含 health，则不能复用（需重算补充 health）。
    let now_max = loader::current_max_tick(db_pool).await.unwrap_or(0);
    {
        let cache_read = CACHE.read().await;
        if let Some(cache) = cache_read.as_ref()
            && let Some(hit) = cache.get(&(tick_start, tick_end))
            && hit.computed_at_tick == now_max
            && (!include_health || hit.result.health.is_some())
        {
            return Ok((*hit.result).clone());
        }
    }

    // 阶段 1 + 2：纯函数检测
    let (rows, agent_names) = loader::fetch_window(db_pool, tick_start, tick_end).await?;
    let (events, candidate_count) = detector::run_detection(&rows, &agent_names, config);

    let causal_count = events.iter().filter(|e| e.category == "causal_emergence").count();
    let co_count = events.iter().filter(|e| e.category == "co_occurrence").count();

    // 可选健康度
    let health = if include_health {
        Some(
            loader::fetch_health(
                db_pool,
                tick_start,
                tick_end,
                &config.health.supply_actions,
                config.health.min_survivors,
                config.health.min_supply_count,
            )
            .await?,
        )
    } else {
        None
    };

    let result = DetectionResult {
        tick_start,
        tick_end,
        candidate_count,
        causal_emergence_count: causal_count,
        co_occurrence_count: co_count,
        events,
        health,
    };

    // 写缓存（同时清理过期条目，防止不同窗口查询累积导致内存无限增长）
    let result_arc = Arc::new(result.clone());
    {
        let mut cache_write = CACHE.write().await;
        let cache = cache_write.get_or_insert_with(HashMap::new);
        // 清理 computed_at_tick 与当前不同的条目（数据已过期，缓存失效）
        cache.retain(|_, v| v.computed_at_tick == now_max);
        cache.insert(
            (tick_start, tick_end),
            CachedResult {
                result: result_arc,
                computed_at_tick: now_max,
            },
        );
    }

    Ok(result)
}

/// 默认窗口：最近 N tick 回溯（MVP 观测窗口 = 240）
pub fn default_window(max_tick: i64, window: i64) -> (i64, i64) {
    let tick_end = max_tick;
    let tick_start = (max_tick - window).max(0);
    (tick_start, tick_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_window() {
        let (s, e) = default_window(1000, 240);
        assert_eq!(s, 760);
        assert_eq!(e, 1000);

        // 不回退到负数
        let (s2, _) = default_window(100, 240);
        assert_eq!(s2, 0);
    }
}
