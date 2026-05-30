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
use crate::models::AgentState;

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
