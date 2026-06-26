// ============================================================================
// Reward Dashboard Handlers（世界生存态势仪表盘）
// ============================================================================
//
// 接口契约：
// GET /api/dashboard/reward/trends           → 寿数趋势 + 死因分布 + 平均 reward
// GET /api/dashboard/reward/lifetime/{id}    → 单 agent 一生 reward 明细
//
// 数据来源：rewards/lifetime/*.jsonl 落盘文件（天道账本）
// ============================================================================

use axum::{Json, extract::{Path, State}, http::StatusCode};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::game_data::registry::RewardRegistry;
use crate::state::AppState;

/// 寿数趋势 + 死因分布响应
#[derive(Serialize)]
pub struct RewardTrends {
    /// 已结算的死亡 agent 总数
    pub total_deaths: usize,
    /// 平均寿数（游戏日）
    pub avg_longevity_days: f64,
    /// 最长寿数（游戏日）
    pub max_longevity_days: i64,
    /// 最短寿数（游戏日）
    pub min_longevity_days: i64,
    /// 平均一生 reward
    pub avg_lifetime_reward: f64,
    /// 死因分布：cause → count
    pub death_cause_distribution: Vec<DeathCauseCount>,
    /// 寿数直方图（按 longevity_days 分桶）
    pub longevity_histogram: Vec<LongevityBucket>,
}

#[derive(Serialize)]
pub struct DeathCauseCount {
    pub cause: String,
    pub count: usize,
}

#[derive(Serialize)]
pub struct LongevityBucket {
    pub range: String,
    pub count: usize,
}

/// GET /api/dashboard/reward/trends
///
/// 聚合所有已落盘的 lifetime reward 记录，输出世界生存态势。
pub async fn get_reward_trends(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RewardTrends>, (StatusCode, String)> {
    let records = load_all_lifetime_records(&state).await?;

    if records.is_empty() {
        return Ok(Json(RewardTrends {
            total_deaths: 0,
            avg_longevity_days: 0.0,
            max_longevity_days: 0,
            min_longevity_days: 0,
            avg_lifetime_reward: 0.0,
            death_cause_distribution: vec![],
            longevity_histogram: vec![],
        }));
    }

    let total = records.len();
    let sum_longevity: i64 = records.iter().map(|r| r.longevity_days).sum();
    let avg_longevity = sum_longevity as f64 / total as f64;
    let max_longevity = records.iter().map(|r| r.longevity_days).max().unwrap_or(0);
    let min_longevity = records.iter().map(|r| r.longevity_days).min().unwrap_or(0);
    let sum_reward: f64 = records.iter().map(|r| r.total).sum();
    let avg_reward = sum_reward / total as f64;

    // 死因分布
    let mut cause_map: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in &records {
        *cause_map.entry(r.death_cause.clone()).or_default() += 1;
    }
    let mut death_cause_distribution: Vec<DeathCauseCount> = cause_map
        .into_iter()
        .map(|(cause, count)| DeathCauseCount { cause, count })
        .collect();
    // 降序：sort_by_key 升序，用 count 的负值实现降序
    death_cause_distribution.sort_by_key(|d| std::cmp::Reverse(d.count));

    // 寿数直方图（每 10 游戏日一桶）
    let longevity_histogram = build_longevity_histogram(&records);

    Ok(Json(RewardTrends {
        total_deaths: total,
        avg_longevity_days: avg_longevity,
        max_longevity_days: max_longevity,
        min_longevity_days: min_longevity,
        avg_lifetime_reward: avg_reward,
        death_cause_distribution,
        longevity_histogram,
    }))
}

/// GET /api/dashboard/reward/lifetime/{id}
///
/// 返回单 agent 的一生 reward 明细。
pub async fn get_agent_lifetime_reward(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Option<crate::reward::LifetimeReward>>, (StatusCode, String)> {
    let records = load_all_lifetime_records(&state).await?;
    let found = records.into_iter().find(|r| r.agent_id == agent_id);
    Ok(Json(found))
}

/// 加载所有 lifetime reward 记录（从落盘 JSONL）。
async fn load_all_lifetime_records(
    _state: &AppState,
) -> Result<Vec<crate::reward::LifetimeReward>, (StatusCode, String)> {
    let cfg = RewardRegistry::get_config().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "reward config not loaded".to_string(),
        )
    })?;

    let lifetime_dir = crate::paths::get_data_dir()
        .join(&cfg.output.base_dir)
        .join("lifetime");

    if !lifetime_dir.exists() {
        return Ok(vec![]);
    }

    let mut records = Vec::new();
    let mut entries = tokio::fs::read_dir(&lifetime_dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        let content = tokio::fs::read_to_string(entry.path())
            .await
            .unwrap_or_default();
        for line in content.lines() {
            if let Ok(record) = serde_json::from_str::<crate::reward::LifetimeReward>(line) {
                records.push(record);
            }
        }
    }

    Ok(records)
}

/// 构建寿数直方图（每 10 游戏日一桶）。
fn build_longevity_histogram(records: &[crate::reward::LifetimeReward]) -> Vec<LongevityBucket> {
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<i64, usize> = BTreeMap::new();
    for r in records {
        let bucket_key = (r.longevity_days / 10) * 10;
        *buckets.entry(bucket_key).or_default() += 1;
    }
    buckets
        .into_iter()
        .map(|(start, count)| LongevityBucket {
            range: format!("{}-{}", start, start + 9),
            count,
        })
        .collect()
}
