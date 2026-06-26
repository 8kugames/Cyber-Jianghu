// ============================================================================
// 一生 Reward 结算（死亡时触发）
// ============================================================================
//
// 寿数（存活游戏日数）+ 统一死亡 penalty。
// 死因仅记录叙事，不参与 penalty 计算（决策③：死就是死，无高下）。
// 幂等：同一 agent 重复触发只覆盖，不重复写（文件名按 agent_id）。
// ============================================================================

use anyhow::{Context, Result};
use uuid::Uuid;

use super::types::LifetimeReward;
use crate::db::DbPool;
use crate::game_data::registry::RewardRegistry;

/// 结算某死亡 agent 的一生 reward。
///
/// 触发点：死亡检测（mutator.rs 置 is_alive=false）之后调用。
/// 幂等：落盘文件按 agent_id 命名，重复触发覆盖不重复。
pub async fn settle_lifetime(pool: &DbPool, dead_agent_id: Uuid) -> Result<LifetimeReward> {
    let cfg =
        RewardRegistry::get_config().ok_or_else(|| anyhow::anyhow!("reward config not loaded"))?;

    // 1. 寿数：birth_tick 与 death_tick
    let (birth_tick, death_tick, character_name) = fetch_lifetime_span(pool, dead_agent_id).await?;
    let ticks_per_day = ticks_per_game_day()?;
    let longevity_days = if ticks_per_day > 0 {
        (death_tick - birth_tick).max(0) / ticks_per_day
    } else {
        0
    };

    // 2. 累积 reward：读该 agent 已落盘的 daily reward 求和（完整日）
    let mut cumulative_reward = load_cumulative_daily(dead_agent_id).await.unwrap_or(0.0);

    // 2.1 缺陷4修复：补算最后一个不完整日的生存奖励（按存活 tick 比例）
    //     settle_daily 只在整日边界触发，日中死亡的 agent 最后一日 daily 未生成。
    //     按 (partial_ticks / ticks_per_day) 比例补生存奖励，忠实"寿数即 reward"。
    let partial_reward = compute_partial_survival(
        birth_tick,
        death_tick,
        ticks_per_day,
        cfg.daily.survival_score,
    );
    cumulative_reward += partial_reward;

    // 3. 死因：从最后一条 agent_states 推断归零属性，再查 DeathInfo
    let (death_cause, death_message) = fetch_death_info(pool, dead_agent_id, death_tick).await?;

    // 4. 统一 penalty（决策③：不分死因）
    let death_penalty = cfg.lifetime.death_penalty;
    let total = cumulative_reward + death_penalty;

    let record = LifetimeReward {
        agent_id: dead_agent_id,
        character_name,
        birth_tick,
        death_tick,
        longevity_days,
        cumulative_reward,
        death_penalty,
        death_cause,
        death_message,
        total,
    };

    if cfg.output.flush_on_death {
        write_lifetime_record(&record).await?;
    }

    tracing::info!(
        "[reward] 一生结算: agent={}, name={}, longevity_days={}, cumulative={}, penalty={}, total={}",
        dead_agent_id,
        record.character_name,
        record.longevity_days,
        record.cumulative_reward,
        record.death_penalty,
        record.total
    );

    Ok(record)
}

/// 每游戏日对应的 tick 数（复用 TimeRegistry，不硬编码 12）。
fn ticks_per_game_day() -> Result<i64> {
    use crate::game_data::registry::TimeRegistry;
    use crate::game_data::registry_or_error;
    let time_cfg =
        TimeRegistry::get_config().ok_or_else(|| anyhow::anyhow!("time config not loaded"))?;
    // registry_or_error 返回 Result<_, String>，转 anyhow
    let registry = registry_or_error().map_err(anyhow::Error::msg)?;
    let real_seconds_per_tick = registry
        .get()
        .game_rules
        .data
        .agent_state
        .tick
        .real_seconds_per_tick as i64;
    Ok(time_cfg.ticks_per_hour as i64 * time_cfg.hours_per_day as i64 * real_seconds_per_tick)
}

/// 取 agent 的 birth_tick / death_tick / name。
async fn fetch_lifetime_span(pool: &DbPool, agent_id: Uuid) -> Result<(i64, i64, String)> {
    use sqlx::Row;
    // death_tick：该 agent 最后一条 agent_states（is_alive=false 或最新）的 tick_id
    // birth_tick：agents 表的 birth_tick 字段
    let row = sqlx::query(
        r#"
        SELECT a.birth_tick, a.name,
               (SELECT MAX(s.tick_id) FROM agent_states s WHERE s.agent_id = a.agent_id) AS death_tick
        FROM agents a
        WHERE a.agent_id = $1
        "#,
    )
    .bind(agent_id)
    .fetch_one(pool)
    .await?;

    let birth_tick: Option<i64> = row.try_get("birth_tick")?;
    let name: String = row.try_get("name")?;
    let death_tick: i64 = row.try_get::<Option<i64>, _>("death_tick")?.unwrap_or(0);
    let birth_tick = birth_tick.unwrap_or(0);

    Ok((birth_tick, death_tick, name))
}

/// 推断死因：从死亡 tick 的状态找归零属性，查 DeathInfo。
async fn fetch_death_info(
    pool: &DbPool,
    agent_id: Uuid,
    death_tick: i64,
) -> Result<(String, String)> {
    use sqlx::Row;
    // 取死亡 tick 的状态（精确到 death_tick，避免读到旧状态）
    let row = sqlx::query(
        r#"SELECT status FROM agent_states WHERE agent_id = $1 AND tick_id = $2 ORDER BY tick_id DESC LIMIT 1"#,
    )
    .bind(agent_id)
    .bind(death_tick)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return unknown_death();
    };

    // status 是 JSON，解析找归零属性（值 <= 0 且有 death_condition）
    let status_json: serde_json::Value =
        serde_json::from_value(row.try_get::<serde_json::Value, _>("status")?).unwrap_or_default();

    // 遍历属性找 <=0 的（候选死因），优先 satiation/hydration/hp
    for attr in ["satiation", "hydration", "hp"] {
        if let Some(val) = status_json.get(attr).and_then(|v| v.as_f64())
            && val <= 0.0
        {
            // 查该属性的 DeathInfo（GameDataCache::get_death_info）
            if let Some(info) =
                crate::game_data::registry::registry().and_then(|cache| cache.get_death_info(attr))
            {
                return Ok((info.cause, info.message));
            }
        }
    }

    unknown_death()
}

fn unknown_death() -> Result<(String, String)> {
    use crate::game_data::registry::registry;
    let info = registry()
        .map(|cache| cache.get_unknown_death_info())
        .ok_or_else(|| anyhow::anyhow!("registry not initialized"))?;
    Ok((info.cause, info.message))
}

/// 读该 agent 已落盘的所有 daily reward，求 cumulative 之和。
async fn load_cumulative_daily(agent_id: Uuid) -> Result<f64> {
    let cfg =
        RewardRegistry::get_config().ok_or_else(|| anyhow::anyhow!("reward config not loaded"))?;
    let daily_dir = crate::paths::get_data_dir()
        .join(&cfg.output.base_dir)
        .join("daily");

    if !daily_dir.exists() {
        return Ok(0.0);
    }

    let mut entries = tokio::fs::read_dir(&daily_dir).await?;
    let mut total = 0.0;
    while let Some(entry) = entries.next_entry().await? {
        let content = tokio::fs::read_to_string(entry.path())
            .await
            .unwrap_or_default();
        for line in content.lines() {
            if let Ok(record) = serde_json::from_str::<super::types::DailyReward>(line)
                && record.agent_id == agent_id
            {
                total += record.total;
            }
        }
    }
    Ok(total)
}

/// 写入一生 reward 记录（按 agent_id 命名，幂等覆盖）。
async fn write_lifetime_record(record: &LifetimeReward) -> Result<()> {
    let cfg =
        RewardRegistry::get_config().ok_or_else(|| anyhow::anyhow!("reward config not loaded"))?;
    let dir = crate::paths::get_data_dir()
        .join(&cfg.output.base_dir)
        .join("lifetime");
    tokio::fs::create_dir_all(&dir).await?;

    let path = dir.join(format!("agent={}.jsonl", record.agent_id));
    let line = serde_json::to_string(record)? + "\n";
    tokio::fs::write(&path, line)
        .await
        .with_context(|| format!("写入 lifetime reward 失败: {:?}", path))?;
    Ok(())
}

/// 纯函数：计算死亡时不足一整日的部分生存奖励（缺陷4修复）。
///
/// settle_daily 只在整日边界触发，日中死亡的 agent 最后一日 daily 未生成。
/// 此函数按存活 tick 比例补算生存奖励，忠实"寿数即 reward"——活多久给多少。
///
/// 数学：partial_ticks = (death - birth) % ticks_per_day（不足一日的部分）
///       partial_reward = survival_score × (partial_ticks / ticks_per_day)
/// 完整日死亡的 agent partial_ticks=0，补算为 0，不重复计入。
pub fn compute_partial_survival(
    birth_tick: i64,
    death_tick: i64,
    ticks_per_day: i64,
    survival_score: f64,
) -> f64 {
    if ticks_per_day <= 0 || death_tick <= birth_tick {
        return 0.0;
    }
    let lifespan = death_tick - birth_tick;
    let partial_ticks = lifespan % ticks_per_day;
    if partial_ticks == 0 {
        return 0.0;
    }
    let ratio = partial_ticks as f64 / ticks_per_day as f64;
    survival_score * ratio
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F4-2 验收：活不过一日的 agent 仍有生存奖励（非恒为 0）
    #[test]
    fn test_partial_survival_short_lived_agent() {
        // 活 100 tick（ticks_per_day=720），survival_score=1.0
        let reward = compute_partial_survival(0, 100, 720, 1.0);
        assert!(
            (reward - (100.0 / 720.0)).abs() < 0.001,
            "活100tick应得 100/720 比例奖励，got {}",
            reward
        );
        assert!(reward > 0.0, "短命 agent 必须有正的生存奖励");
    }

    /// F4-1 验收：活半日的 agent 按比例得奖励
    #[test]
    fn test_partial_survival_half_day() {
        // 活 360 tick（半日），应得 0.5
        let reward = compute_partial_survival(0, 360, 720, 1.0);
        assert!(
            (reward - 0.5).abs() < 0.001,
            "活半日应得 0.5，got {}",
            reward
        );
    }

    /// 完整日死亡的 agent 补算为 0（不重复计入）
    #[test]
    fn test_partial_survival_exact_day_is_zero() {
        let reward = compute_partial_survival(0, 720, 720, 1.0);
        assert!(
            (reward - 0.0).abs() < 0.001,
            "完整日死亡补算应为0，got {}",
            reward
        );
    }

    /// 多日 + 不完整尾部的 agent
    #[test]
    fn test_partial_survival_multi_day_plus_partial() {
        // 活 720*3 + 360 = 2520 tick（3.5 日）
        let reward = compute_partial_survival(0, 2520, 720, 1.0);
        assert!(
            (reward - 0.5).abs() < 0.001,
            "3.5日的尾部应得0.5，got {}",
            reward
        );
    }
}
