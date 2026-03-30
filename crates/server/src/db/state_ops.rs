// ============================================================================
// OpenClaw Cyber-Jianghu MVP AgentState数据库操作模块
// ============================================================================
//
// 本模块实现AgentState相关的数据库操作，包括：
// - 创建Agent状态记录
// - 查询Agent状态（最新状态、所有存活Agent状态）
// - 批量插入Agent状态（Tick引擎核心）
// - Tick日志操作
// - Agent动作日志操作

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, QueryBuilder};
use tracing::{debug, error, info};

use crate::models::{AgentAction, AgentState, TickLog};

// ============================================================================
// AgentState 相关操作
// ============================================================================

/// 获取所有存活Agent的最新状态
///
/// 这是Tick引擎在阶段2（加载Agent状态）时使用的核心函数
///
/// # 参数
/// - pool: 数据库连接池
///
/// # 返回
/// - `Ok(Vec<AgentState>)`: 所有存活Agent的最新状态列表
/// - Err: 查询失败
pub async fn get_all_alive_agents_latest_states(pool: &PgPool) -> Result<Vec<AgentState>> {
    debug!("查询所有存活Agent的最新状态");

    // 先取每个 agent 的最新记录，再过滤 is_alive
    // 不能在 DISTINCT ON 之前加 WHERE is_alive = true，
    // 否则会忽略最新的死亡记录、找到旧的存活记录，导致已死亡 agent 被加载
    let states = sqlx::query_as::<Postgres, AgentState>(
        r#"
        SELECT * FROM (
            SELECT DISTINCT ON (agent_id) *
            FROM agent_states
            ORDER BY agent_id, tick_id DESC
        ) latest
        WHERE is_alive = true
        "#,
    )
    .fetch_all(pool)
    .await
    .context("获取所有存活 Agent 最新状态失败")?;

    debug!("查询到 {} 个存活Agent", states.len());
    Ok(states)
}

/// 获取当前世界Tick ID
///
/// 使用 tick_logs 与 agent_states 的最大值，保证重启后 Tick 不回退。
pub async fn get_current_world_tick_id(pool: &PgPool) -> Result<i64> {
    let tick_id: i64 = sqlx::query_scalar(
        r#"
        SELECT GREATEST(
            COALESCE((SELECT MAX(tick_id) FROM tick_logs), 0),
            COALESCE((SELECT MAX(tick_id) FROM agent_states), 0)
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .context("获取当前世界 tick ID 失败")?;

    Ok(tick_id)
}

/// 获取最新状态快照Tick ID
///
/// 仅使用 agent_states 的最大值，适用于按状态快照查询。
pub async fn get_latest_state_tick_id(pool: &PgPool) -> Result<i64> {
    let tick_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(tick_id), 0) FROM agent_states")
        .fetch_one(pool)
        .await
        .context("获取最新状态 tick ID 失败")?;

    Ok(tick_id)
}

/// 获取指定 Agent 的最新状态
///
/// # 参数
/// - pool: 数据库连接池
/// - agent_id: Agent ID
///
/// # 返回
/// - Ok(AgentState): Agent 的最新状态
/// - Err: 查询失败或 Agent 不存在
pub async fn get_latest_agent_state(pool: &PgPool, agent_id: uuid::Uuid) -> Result<AgentState> {
    debug!("获取 Agent {} 的最新状态", agent_id);

    let state = sqlx::query_as::<Postgres, AgentState>(
        r#"
        SELECT * FROM agent_states
        WHERE agent_id = $1
        ORDER BY tick_id DESC
        LIMIT 1
        "#,
    )
    .bind(agent_id)
    .fetch_one(pool)
    .await
    .context(format!("获取 Agent {} 最新状态失败", agent_id))?;

    Ok(state)
}

/// 获取最后一次Tick的时间戳
///
/// 用于计算当前Tick的进度，实现平滑时间插值。
pub async fn get_last_tick_time(pool: &PgPool) -> Result<DateTime<Utc>> {
    let timestamp: DateTime<Utc> = sqlx::query_scalar(
        "SELECT COALESCE(MAX(created_at), NOW() AT TIME ZONE 'UTC') FROM tick_logs",
    )
    .fetch_one(pool)
    .await
    .context("获取最后tick时间戳失败")?;

    Ok(timestamp)
}

/// 批量插入Agent状态
///
/// 用于Tick引擎在阶段5（持久化状态）时批量保存所有Agent的状态
///
/// # 参数
/// - pool: 数据库连接池
/// - states: Agent状态列表
///
/// # 返回
/// - Ok(()): 插入成功
/// - Err: 插入失败（包括序列化失败）
pub async fn batch_insert_agent_states(pool: &PgPool, states: &[AgentState]) -> Result<()> {
    if states.is_empty() {
        debug!("没有状态需要插入");
        return Ok(());
    }

    info!("批量插入 {} 个Agent状态", states.len());

    // F-05: 预先序列化所有属性，失败时立即返回错误（禁止静默吞掉）
    let serialized: Vec<(uuid::Uuid, i64, serde_json::Value, String, bool)> = states
        .iter()
        .map(|state| {
            let attributes_json = serde_json::to_value(state.get_attributes_for_protocol())
                .map_err(|e| {
                    error!("序列化 Agent {} 属性失败: {}", state.agent_id, e);
                    anyhow::anyhow!("F-05: Agent {} 属性序列化失败: {}", state.agent_id, e)
                })?;
            Ok((
                state.agent_id,
                state.tick_id,
                attributes_json,
                state.node_id.clone(),
                state.is_alive,
            ))
        })
        .collect::<Result<Vec<_>>>()?;

    // 使用QueryBuilder构建批量插入SQL - 完全动态 JSONB
    let mut query_builder: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO agent_states (agent_id, tick_id, attributes, node_id, is_alive) ",
    );

    query_builder.push_values(
        serialized,
        |mut b, (agent_id, tick_id, attributes_json, node_id, is_alive)| {
            b.push_bind(agent_id)
                .push_bind(tick_id)
                .push_bind(attributes_json)
                .push_bind(node_id)
                .push_bind(is_alive);
        },
    );

    query_builder
        .build()
        .execute(pool)
        .await
        .map_err(|e| {
            error!("批量插入Agent状态失败: {}", e);
            error!("States count: {}", states.len());
            e
        })
        .context("批量插入 Agent 状态失败")?;

    info!("批量插入完成");
    Ok(())
}

// ============================================================================
// Tick日志相关操作
// ============================================================================

/// 创建Tick日志
///
/// # 参数
/// - pool: 数据库连接池
/// - tick_log: Tick日志
///
/// # 返回
/// - Ok(TickLog): 创建的Tick日志
/// - Err: 创建失败
pub async fn create_tick_log(pool: &PgPool, tick_log: &TickLog) -> Result<TickLog> {
    debug!("创建Tick日志: tick_id={}", tick_log.tick_id);

    // 插入记录并返回 tick_id 用于验证
    let returned_tick_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO tick_logs (
            tick_id, started_at, status
        )
        VALUES ($1, $2, $3)
        RETURNING tick_id
        "#,
    )
    .bind(tick_log.tick_id)
    .bind(tick_log.started_at)
    .bind(tick_log.status.to_string())
    .fetch_one(pool)
    .await
    .context("创建 tick 日志失败")?;

    // 验证返回的 tick_id 与预期一致
    if returned_tick_id != tick_log.tick_id {
        return Err(anyhow::anyhow!(
            "Tick ID mismatch: expected {}, got {}",
            tick_log.tick_id,
            returned_tick_id
        ));
    }

    Ok(tick_log.clone())
}

/// 更新Tick日志
///
/// # 参数
/// - pool: 数据库连接池
/// - tick_log: Tick日志
///
/// # 返回
/// - Ok(()): 更新成功
/// - Err: 更新失败
pub async fn update_tick_log(pool: &PgPool, tick_log: &TickLog) -> Result<()> {
    debug!("更新Tick日志: tick_id={}", tick_log.tick_id);

    sqlx::query(
        r#"
        UPDATE tick_logs
        SET completed_at = $2,
            duration_ms = $3,
            agents_processed = $4,
            actions_executed = $5,
            status = $6,
            error_message = $7
        WHERE tick_id = $1
        "#,
    )
    .bind(tick_log.tick_id)
    .bind(tick_log.completed_at)
    .bind(tick_log.duration_ms)
    .bind(tick_log.agents_processed)
    .bind(tick_log.actions_executed)
    .bind(tick_log.status.to_string())
    .bind(&tick_log.error_message)
    .execute(pool)
    .await
    .context("更新 tick 日志失败")?;

    Ok(())
}

// ============================================================================
// Agent动作日志相关操作
// ============================================================================

/// 批量插入Agent动作日志
///
/// # 参数
/// - pool: 数据库连接池
/// - actions: Agent动作列表
///
/// # 返回
/// - Ok(()): 插入成功
/// - Err: 插入失败
pub async fn batch_insert_action_logs(pool: &PgPool, actions: &[AgentAction]) -> Result<()> {
    if actions.is_empty() {
        debug!("没有动作日志需要插入");
        return Ok(());
    }

    debug!("批量插入 {} 个动作日志", actions.len());

    let mut query_builder: QueryBuilder<Postgres> = QueryBuilder::new(
        "INSERT INTO agent_action_logs (tick_id, agent_id, action_type, action_data, result, thought_log, observer_thought, narrative) ",
    );

    query_builder.push_values(actions, |mut b, action| {
        b.push_bind(action.tick_id)
            .push_bind(action.agent_id)
            .push_bind(action.action_type.to_string())
            .push_bind(&action.action_data)
            .push_bind(action.result.to_string())
            .push_bind(&action.thought_log)
            .push_bind(&action.observer_thought)
            .push_bind(&action.narrative);
    });

    query_builder
        .build()
        .execute(pool)
        .await
        .context("批量插入动作日志失败")?;

    debug!("批量插入动作日志完成");
    Ok(())
}
