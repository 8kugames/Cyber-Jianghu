// ============================================================================
// 周期 Reward 聚合（复用 chronicle 7 游戏日周期）
// ============================================================================
//
// 聚合周期内每个存活 agent 的每日 reward 总和，落盘 periodic/period=<start>_<end>.jsonl
// 不重复计算 reward，只读已落盘的 daily 记录聚合。
// ============================================================================

use anyhow::{Context, Result};
use uuid::Uuid;

use super::types::PeriodReward;
use crate::db::DbPool;
use crate::game_data::registry::RewardRegistry;

/// 结算某周期内所有 agent 的聚合 reward。
///
/// 触发点：scheduler.rs 周期分支（复用 chronicle period_ticks）。
/// 旁路调用：失败只 error 日志，不阻断 tick。
pub async fn settle_periodic(
    pool: &DbPool,
    period_start: i64,
    period_end: i64,
) -> Result<Vec<PeriodReward>> {
    // 读周期内存活 agent（用 period_end 时刻的状态判断存活）
    let agents = crate::db::get_all_alive_agents_latest_states(pool).await?;

    // 聚合每个 agent 的 daily reward
    let mut records = Vec::with_capacity(agents.len());
    for agent in &agents {
        let cumulative = aggregate_daily_for_agent(agent.agent_id, period_start, period_end).await;
        records.push(PeriodReward {
            agent_id: agent.agent_id,
            day_start: period_start,
            day_end: period_end,
            cumulative_daily_reward: cumulative,
            survived_period: agent.is_alive,
        });
    }

    // 落盘
    if !records.is_empty() {
        write_periodic_batch(&records, period_start, period_end).await?;
    }

    tracing::info!(
        "[reward] 周期聚合完成: period={}~{}, agents={}",
        period_start,
        period_end,
        records.len()
    );

    Ok(records)
}

/// 聚合某 agent 在 [period_start, period_end] tick 范围内的 daily reward 总和。
///
/// 从已落盘的 daily/*.jsonl 读取（不重复计算），按 tick_id 范围过滤。
async fn aggregate_daily_for_agent(agent_id: Uuid, period_start: i64, period_end: i64) -> f64 {
    let cfg = match RewardRegistry::get_config() {
        Some(c) => c,
        None => return 0.0,
    };
    let daily_dir = crate::paths::get_data_dir()
        .join(&cfg.output.base_dir)
        .join("daily");

    if !daily_dir.exists() {
        return 0.0;
    }

    let mut total = 0.0;
    let mut entries = match tokio::fs::read_dir(&daily_dir).await {
        Ok(e) => e,
        Err(_) => return 0.0,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let content = tokio::fs::read_to_string(entry.path())
            .await
            .unwrap_or_default();
        for line in content.lines() {
            if let Ok(record) = serde_json::from_str::<super::types::DailyReward>(line)
                && record.agent_id == agent_id
                && record.tick_id >= period_start
                && record.tick_id <= period_end
            {
                total += record.total;
            }
        }
    }
    total
}

/// 批量写入周期 reward 到 JSONL。
///
/// 路径：<data_base_dir>/rewards/periodic/period=<start>_<end>.jsonl
async fn write_periodic_batch(
    records: &[PeriodReward],
    period_start: i64,
    period_end: i64,
) -> Result<()> {
    let cfg =
        RewardRegistry::get_config().ok_or_else(|| anyhow::anyhow!("reward config not loaded"))?;
    if !cfg.output.enabled {
        return Ok(());
    }

    let dir = crate::paths::get_data_dir()
        .join(&cfg.output.base_dir)
        .join("periodic");
    tokio::fs::create_dir_all(&dir).await?;

    let path = dir.join(format!("period={}_{}.jsonl", period_start, period_end));
    let mut content = String::new();
    for r in records {
        content.push_str(&serde_json::to_string(r)?);
        content.push('\n');
    }

    tokio::fs::write(&path, content)
        .await
        .with_context(|| format!("写入 periodic reward 失败: {:?}", path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::init_test_registry;

    /// P2-1 验收：周期聚合应等于周期内 daily 之和。
    /// 此测试验证聚合逻辑的正确性（mock daily 文件）。
    #[tokio::test]
    async fn test_aggregate_daily_for_agent_sums_records() {
        init_test_registry();

        let tmp = tempfile::TempDir::new().unwrap();
        unsafe {
            std::env::set_var("CYBER_JIANGHU_DATA_DIR", tmp.path());
        }

        let agent_id = Uuid::new_v4();
        let daily_dir = tmp.path().join("rewards").join("daily");
        tokio::fs::create_dir_all(&daily_dir).await.unwrap();

        // 写两条 daily 记录（一条在范围内，一条在范围外）
        let in_range = super::super::types::DailyReward {
            agent_id,
            tick_id: 750,
            game_day: 1,
            survival: 1.0,
            physiological: 0.3,
            tianhun_judgment: None,
            total: 1.3,
        };
        let out_range = super::super::types::DailyReward {
            agent_id,
            tick_id: 5000,
            game_day: 10,
            survival: 1.0,
            physiological: 0.3,
            tianhun_judgment: None,
            total: 1.3,
        };
        let mut content = String::new();
        content.push_str(&serde_json::to_string(&in_range).unwrap());
        content.push('\n');
        content.push_str(&serde_json::to_string(&out_range).unwrap());
        content.push('\n');
        tokio::fs::write(daily_dir.join("day=1.jsonl"), content)
            .await
            .unwrap();

        // 聚合 [700, 800] 范围：应只包含 in_range (tick=750)
        let total = aggregate_daily_for_agent(agent_id, 700, 800).await;
        assert!(
            (total - 1.3).abs() < 0.001,
            "应只聚合范围内的记录，got {}",
            total
        );
    }
}
