// ============================================================================
// OpenClaw Cyber-Jianghu MVP - Persistence Module
// ============================================================================
//
// 本模块负责Agent状态的持久化操作
//
// 功能：
// - 从数据库加载Agent状态
// - 批量保存Agent状态到数据库
// - 保存Tick日志
// ============================================================================

use anyhow::{Context, Result};
use tracing::debug;

use crate::db::DbPool;
use crate::models::{AgentState, TickLog};

/// 从数据库加载所有Agent的当前状态
///
/// 查询所有存活的Agent的最新状态
pub async fn load_agent_states(db_pool: &DbPool) -> Result<Vec<AgentState>> {
    debug!("查询所有存活Agent的最新状态");

    // 调用数据库操作函数
    let states = crate::db::get_all_alive_agents_latest_states(db_pool)
        .await
        .context("从数据库加载Agent状态失败")?;

    debug!("加载了 {} 个Agent状态", states.len());
    Ok(states)
}

/// 持久化状态到数据库
///
/// 批量插入Agent状态到数据库
pub async fn persist_states(
    db_pool: &DbPool,
    tick_id: i64,
    agent_states: &[AgentState],
) -> Result<()> {
    if agent_states.is_empty() {
        debug!("Tick {}: 没有状态需要持久化", tick_id);
        return Ok(());
    }

    debug!(
        "Tick {}: 持久化 {} 个Agent状态",
        tick_id,
        agent_states.len()
    );

    // 更新每个状态的 tick_id 为当前 tick
    let states_with_tick: Vec<AgentState> = agent_states
        .iter()
        .map(|s| {
            let mut state = s.clone();
            state.tick_id = tick_id;
            state
        })
        .collect();

    // 批量插入Agent状态
    crate::db::batch_insert_agent_states(db_pool, &states_with_tick)
        .await
        .context("批量插入Agent状态失败")?;

    debug!("Tick {}: 状态持久化完成", tick_id);
    Ok(())
}

/// 保存Tick日志到数据库
///
/// MVP阶段：只打印日志，不实际保存
/// TODO: 实现真实的数据库保存（Phase 2）
pub async fn save_tick_log(tick_log: &TickLog) -> Result<()> {
    // MVP阶段：只打印日志，不实际保存到数据库
    // Phase 2 将实现真实的数据库保存

    tracing::info!(
        "Tick日志: id={}, status={:?}, agents={}, actions={}, duration={}ms",
        tick_log.tick_id,
        tick_log.status,
        tick_log.agents_processed,
        tick_log.actions_executed,
        tick_log.duration_ms.unwrap_or(0)
    );

    Ok(())
}
