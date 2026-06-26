// ============================================================================
// 每日 Reward 计算
// ============================================================================
//
// 每游戏日末批量结算所有存活 agent 的当日 reward。
// 所有系数从 RewardRegistry 读取，max_value 从 StateRegistry 读取，零硬编码。
// ============================================================================

use crate::game_data::registry::RewardRegistry;
use crate::game_data::registry::StateRegistry;
use crate::models::AgentState;

use super::types::DailyReward;

/// 计算单个 agent 当日 reward。
///
/// 所有数值来自 RewardConfig（cfg）与 StateRegistry（max_value），零硬编码。
/// 证明见 P1-2 来源断言式测试：篡改 cfg.survival_score 输出随之改变。
pub fn compute_daily_reward(
    agent_state: &AgentState,
    game_day: i64,
    tianhun_result: Option<&str>,
) -> Option<DailyReward> {
    let cfg = RewardRegistry::get_config()?;

    // 1. 生存分量：当日存活即得
    let survival = if agent_state.is_alive {
        cfg.daily.survival_score
    } else {
        0.0
    };

    // 2. 生理分量：satiation/hydration 归一化
    //    max_value 从 StateRegistry 读（复用 StatusComponent::evaluate_max_value 原语），非硬编码
    let satiation_max = StateRegistry::get_attribute_max_value("satiation")? as f64;
    let hydration_max = StateRegistry::get_attribute_max_value("hydration")? as f64;
    let satiation = agent_state.status.get("satiation").unwrap_or(0) as f64;
    let hydration = agent_state.status.get("hydration").unwrap_or(0) as f64;
    let physiological = (satiation / satiation_max * cfg.daily.physiological.satiation_weight)
        + (hydration / hydration_max * cfg.daily.physiological.hydration_weight);

    // 3. 天魂审查分量（P1 阶段 server 读不到 agent 端 soul_cycle.db，judgment 暂为 None）
    let tianhun_judgment = match tianhun_result {
        Some("approved") => Some(cfg.daily.tianhun.approved_score),
        Some("rejected") => Some(cfg.daily.tianhun.rejected_score),
        _ => None,
    };

    let total = survival + physiological + tianhun_judgment.unwrap_or(0.0);

    Some(DailyReward {
        agent_id: agent_state.agent_id,
        tick_id: agent_state.tick_id,
        game_day,
        survival,
        physiological,
        tianhun_judgment,
        total,
    })
}

/// 批量计算所有存活 agent 当日 reward 并落盘。
///
/// 旁路调用：失败只 error 日志，不阻断 tick 主循环（P1-8 验收）。
pub async fn settle_daily(
    pool: &crate::db::DbPool,
    game_day: i64,
    tick_id: i64,
) -> anyhow::Result<Vec<DailyReward>> {
    // 读所有存活 agent 的最新状态（state_ops 通过 crate::db 重导出）
    let agents = crate::db::get_all_alive_agents_latest_states(pool).await?;

    let mut records = Vec::with_capacity(agents.len());
    for agent in &agents {
        // P1 阶段无天魂结果（server 读不到 agent 端），judgment 暂为 None
        if let Some(reward) = compute_daily_reward(agent, game_day, None) {
            records.push(reward);
        }
    }

    // 落盘
    if !records.is_empty() {
        write_daily_batch(&records, tick_id).await?;
    }

    tracing::info!(
        "[reward] 每日结算完成: game_day={}, tick={}, agents={}",
        game_day,
        tick_id,
        records.len()
    );

    Ok(records)
}

/// 批量写入每日 reward 到 JSONL。
///
/// 路径：<data_base_dir>/rewards/daily/day=<game_day>.jsonl
async fn write_daily_batch(records: &[DailyReward], _tick_id: i64) -> anyhow::Result<()> {
    let cfg =
        RewardRegistry::get_config().ok_or_else(|| anyhow::anyhow!("reward config not loaded"))?;
    if !cfg.output.enabled {
        return Ok(());
    }

    let base = crate::paths::get_data_dir()
        .join(&cfg.output.base_dir)
        .join("daily");
    tokio::fs::create_dir_all(&base).await?;

    // 同一 game_day 追加到同一文件
    let game_day = records.first().map(|r| r.game_day).unwrap_or(0);
    let path = base.join(format!("day={}.jsonl", game_day));

    let mut content = String::new();
    for r in records {
        content.push_str(&serde_json::to_string(r)?);
        content.push('\n');
    }

    tokio::fs::write(&path, content).await?;
    Ok(())
}
