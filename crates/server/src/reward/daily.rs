// ============================================================================
// 每日 Reward 计算
// ============================================================================
//
// 每游戏日末批量结算所有存活 agent 的当日 reward。
// 所有系数从 RewardRegistry 读取，max_value 从 StateRegistry 读取，零硬编码。
// ============================================================================

use crate::db::DbPool;
use crate::game_data::registry::RewardRegistry;
use crate::game_data::registry::StateRegistry;
use crate::models::AgentState;
use crate::state::AgentStateCache;

use super::types::DailyReward;

/// 计算单个 agent 当日 reward（纯函数，可单测）。
///
/// 所有数值来自 RewardConfig（cfg）与 StateRegistry（max_value），零硬编码。
/// tianhun_result 来自 agent_action_logs.soul_cycle_metadata（agent 已上报）。
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

    // 3. 天魂审查分量（数据源：agent_action_logs.soul_cycle_metadata，agent 已上报到 server）
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

/// 纯函数：从 soul_cycle_metadata JSON 解析天魂审查结果。
///
/// 数据结构（protocol/messages.rs）：
///   SoulCycleMetadata { cycles: Vec<SoulCycleAttempt>, ... }
///   SoulCycleAttempt { tianhun: TianhunReport, ... }
///   TianhunReport { result: Option<String>, ... }  // "approved"/"rejected"
///
/// 取最后一个 attempt 的 tianhun.result（当日最终认知状态）。
/// 提取为纯函数便于单测（F1-1 验收：approved/rejected/None 三类边界）。
pub fn parse_tianhun_result(metadata_json: &serde_json::Value) -> Option<String> {
    let cycles = metadata_json.get("cycles")?.as_array()?;
    let last_attempt = cycles.last()?;
    let tianhun = last_attempt.get("tianhun")?;
    let result = tianhun.get("result")?.as_str()?;
    if result.is_empty() {
        None
    } else {
        Some(result.to_string())
    }
}

/// 批量计算所有存活 agent 当日 reward 并落盘。
///
/// 数据源：agent_state_cache（DashMap，内存，含当 tick 最新状态）——与 broadcast_new_tick 同源，
/// 消除读 PostgreSQL 的时序竞态（缺陷5修复）。
/// 天魂结果：从 agent_action_logs.soul_cycle_metadata 查询（缺陷1修复）。
///
/// 旁路调用：失败只 error 日志，不阻断 tick 主循环（P1-8 验收）。
pub async fn settle_daily(
    pool: &DbPool,
    state_cache: &AgentStateCache,
    game_day: i64,
    tick_id: i64,
    day_start_tick: i64,
) -> anyhow::Result<Vec<DailyReward>> {
    // 检查配置可用性（缺陷6修复：get_config 失败时显式报错而非静默）
    if RewardRegistry::get_config().is_none() {
        tracing::error!(
            "[reward] settle_daily 跳过：reward 配置未加载（registry 未初始化），game_day={}",
            game_day
        );
        return Ok(vec![]);
    }

    // 读所有存活 agent 的最新状态（DashMap，内存——与 broadcast 同源，消除时序竞态）
    let agents: Vec<AgentState> = state_cache
        .iter()
        .map(|r| r.value().clone())
        .filter(|s| s.is_alive)
        .collect();

    let mut records = Vec::with_capacity(agents.len());
    for agent in &agents {
        // 查询当日天魂审查结果（缺陷1修复：接入已上报数据）
        let tianhun_result =
            fetch_tianhun_result_for_day(pool, agent.agent_id, day_start_tick, tick_id)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("[reward] 查询天魂结果失败 agent={}: {}", agent.agent_id, e);
                    None
                });

        // get_attribute_max_value 失败时 warn（缺陷6修复：覆盖同模式静默路径）
        let reward = compute_daily_reward(agent, game_day, tianhun_result.as_deref());
        match reward {
            Some(r) => records.push(r),
            None => {
                tracing::warn!(
                    "[reward] compute_daily_reward 返回 None agent={}（get_config 或 get_attribute_max_value 失败）",
                    agent.agent_id
                );
            }
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

/// 查询某 agent 在 [day_start_tick, day_end_tick] 内的最终天魂审查结果。
///
/// 数据来源：agent_action_logs.soul_cycle_metadata（agent 通过 SoulCycleReport 上报，server 已存储）。
/// 取该范围内最大 tick_id + pipe_seq 的记录，解析其 cycles[last].tianhun.result。
pub async fn fetch_tianhun_result_for_day(
    pool: &DbPool,
    agent_id: uuid::Uuid,
    day_start_tick: i64,
    day_end_tick: i64,
) -> anyhow::Result<Option<String>> {
    let row = sqlx::query_scalar::<_, Option<serde_json::Value>>(
        r#"SELECT soul_cycle_metadata FROM agent_action_logs
           WHERE agent_id = $1 AND tick_id BETWEEN $2 AND $3
             AND soul_cycle_metadata IS NOT NULL
           ORDER BY tick_id DESC, pipe_seq DESC LIMIT 1"#,
    )
    .bind(agent_id)
    .bind(day_start_tick)
    .bind(day_end_tick)
    .fetch_optional(pool)
    .await?;

    match row {
        Some(Some(metadata)) => Ok(parse_tianhun_result(&metadata)),
        _ => Ok(None),
    }
}

/// 单条追加 daily reward 记录（供 lifetime 死亡补算用）。
pub async fn append_daily_record(record: &DailyReward) {
    let cfg = match RewardRegistry::get_config() {
        Some(c) if c.output.enabled => c,
        _ => return,
    };
    let base = crate::paths::get_data_dir().join(&cfg.output.base_dir).join("daily");
    if tokio::fs::create_dir_all(&base).await.is_err() {
        return;
    }
    let path = base.join(format!("day={}.jsonl", record.game_day));
    let line = match serde_json::to_string(record) {
        Ok(s) => s + "\n",
        Err(_) => return,
    };
    use tokio::io::AsyncWriteExt;
    if let Ok(mut f) = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await {
        let _ = f.write_all(line.as_bytes()).await;
    }
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
