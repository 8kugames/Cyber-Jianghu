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
use serde::Serialize;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use tracing::{debug, error, info, warn};

use crate::models::{AgentAction, AgentState, TickLog};

/// Agent 每日摘要存档（数据库行结构）
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AgentDailySummary {
    pub id: i64,
    pub agent_id: uuid::Uuid,
    pub game_day: i64,
    pub summary: String,
    pub created_at: i64,
}

/// 序列化属性为 JSONB，包含 _skills 数组
///
/// `get_attributes_for_protocol()` 返回 `HashMap<String, i32>`（纯数值），
/// 此 helper 在序列化后注入 `_skills` 键（字符串数组）。
/// `from_row()` 中 `as_i64()` 循环天然跳过非数值类型，零冲突。
pub(super) fn serialize_attributes_with_skills(state: &AgentState) -> Result<serde_json::Value> {
    let mut json = serde_json::to_value(state.get_attributes_for_protocol())
        .map_err(|e| anyhow::anyhow!("Agent {} 属性序列化失败: {}", state.agent_id, e))?;
    if !state.skills.is_empty() {
        json["_skills"] = serde_json::to_value(&state.skills)
            .map_err(|e| anyhow::anyhow!("Agent {} skills 序列化失败: {}", state.agent_id, e))?;
    }
    if !state.status.max_modifiers.is_empty() {
        json["_max_modifiers"] = serde_json::to_value(&state.status.max_modifiers)?;
    }
    if !state.action_counts.is_empty() {
        json["_action_counts"] = serde_json::to_value(&state.action_counts)?;
    }
    Ok(json)
}

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
        SELECT latest.*, a.birth_tick, a.name FROM (
            SELECT DISTINCT ON (agent_id) *
            FROM agent_states
            ORDER BY agent_id, tick_id DESC
        ) latest
        JOIN agents a ON a.agent_id = latest.agent_id
        WHERE latest.is_alive = true
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

    // 空库兜底：tick_logs 和 agent_states 均为空时返回 0，
    // 导致注册时 birth_tick 为负值（BUG-8）。
    // 使用当前 Unix 时间戳作为合理的初始 tick_id。
    if tick_id == 0 {
        return Ok(chrono::Utc::now().timestamp());
    }

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
        SELECT s.*, a.birth_tick, a.name
        FROM agent_states s
        JOIN agents a ON a.agent_id = s.agent_id
        WHERE s.agent_id = $1
        ORDER BY s.tick_id DESC
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

    // 显式事务：保证原子性，防御未来扩展为多语句操作
    let mut tx = pool.begin().await.context("开启事务失败")?;

    // F-05: 预先序列化所有属性，失败时立即返回错误（禁止静默吞掉）
    let serialized: Vec<(uuid::Uuid, i64, serde_json::Value, String, bool)> = states
        .iter()
        .map(|state| {
            let attributes_json = serialize_attributes_with_skills(state)?;
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
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            error!("批量插入Agent状态失败: {}", e);
            error!("States count: {}", states.len());
            e
        })
        .context("批量插入 Agent 状态失败")?;

    tx.commit().await.context("提交事务失败")?;

    info!("批量插入完成");
    Ok(())
}

/// 单条 Agent 状态持久化（实时模式用）
///
/// UPSERT 语义：同 (agent_id, tick_id) 时更新，否则插入。
/// 用于 IntentWorker 的 per-intent 状态持久化。
pub async fn upsert_agent_state(pool: &PgPool, state: &AgentState) -> Result<()> {
    let attributes_json = serialize_attributes_with_skills(state)?;

    sqlx::query(
        r#"
        INSERT INTO agent_states (agent_id, tick_id, attributes, node_id, is_alive)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (agent_id, tick_id) DO UPDATE SET
            attributes = EXCLUDED.attributes,
            node_id = EXCLUDED.node_id,
            is_alive = EXCLUDED.is_alive
        "#,
    )
    .bind(state.agent_id)
    .bind(state.tick_id)
    .bind(attributes_json)
    .bind(&state.node_id)
    .bind(state.is_alive)
    .execute(pool)
    .await
    .context(format!("单条 UPSERT Agent {} 状态失败", state.agent_id))?;

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
        "INSERT INTO agent_action_logs (tick_id, agent_id, action_type, action_type_display, action_data, result, result_message, thought_log, observer_thought, narrative, soul_cycle_metadata, chaos_marker, dream_marker, pipe_seq) ",
    );

    query_builder.push_values(actions, |mut b, action| {
        b.push_bind(action.tick_id)
            .push_bind(action.agent_id)
            .push_bind(action.action_type.to_string())
            .push_bind(&action.action_type_display)
            .push_bind(&action.action_data)
            .push_bind(action.result.to_string())
            .push_bind(&action.result_message)
            .push_bind(&action.thought_log)
            .push_bind(&action.observer_thought)
            .push_bind(&action.narrative)
            .push_bind(&action.soul_cycle_metadata)
            .push_bind(&action.chaos_marker)
            .push_bind(&action.dream_marker)
            .push_bind(action.pipe_seq);
    });

    // UPSERT: SoulCycleReport 可能先到达创建占位行，此处覆盖
    query_builder.push(
        " ON CONFLICT (agent_id, tick_id, pipe_seq) DO UPDATE SET \
         action_type = EXCLUDED.action_type, \
         action_type_display = EXCLUDED.action_type_display, \
         action_data = EXCLUDED.action_data, \
         result = EXCLUDED.result, \
         result_message = EXCLUDED.result_message, \
         thought_log = EXCLUDED.thought_log, \
         observer_thought = EXCLUDED.observer_thought, \
         narrative = EXCLUDED.narrative, \
         chaos_marker = EXCLUDED.chaos_marker, \
         dream_marker = EXCLUDED.dream_marker",
    );

    query_builder
        .build()
        .execute(pool)
        .await
        .context("批量插入动作日志失败")?;

    debug!("批量插入动作日志完成");
    Ok(())
}

/// 更新指定 tick 的三魂循环元数据
///
/// 由 agent 在 intent 发送后通过 WebSocket SoulCycleReport 消息上报。
/// 由于 agent_action_logs 已在 tick 结算时插入，此处执行 UPDATE。
pub async fn update_soul_cycle_metadata(
    pool: &PgPool,
    agent_id: uuid::Uuid,
    tick_id: i64,
    metadata: &serde_json::Value,
) -> Result<()> {
    let rows = sqlx::query(
        "UPDATE agent_action_logs SET soul_cycle_metadata = $1
         WHERE agent_id = $2 AND tick_id = $3",
    )
    .bind(metadata)
    .bind(agent_id)
    .bind(tick_id)
    .execute(pool)
    .await
    .context("更新三魂循环元数据失败")?;

    if rows.rows_affected() == 0 {
        warn!(
            "未找到 agent_action_logs 记录，插入新记录：agent_id={}, tick_id={}",
            agent_id, tick_id
        );
        // Upsert：SoulCycleReport 可能先于 tick processor 到达
        // 提供默认值以满足 NOT NULL 约束（action_type, tick_id FK 已移除）
        sqlx::query(
            "INSERT INTO agent_action_logs (agent_id, tick_id, action_type, result, soul_cycle_metadata)
             VALUES ($1, $2, 'idle', 'success', $3)
             ON CONFLICT (agent_id, tick_id) DO UPDATE SET soul_cycle_metadata = EXCLUDED.soul_cycle_metadata",
        )
        .bind(agent_id)
        .bind(tick_id)
        .bind(metadata)
        .execute(pool)
        .await
        .context("插入三魂循环元数据失败")?;
    } else {
        debug!(
            "已更新 agent_id={}, tick_id={} 的三魂循环元数据",
            agent_id, tick_id
        );
    }

    Ok(())
}

// ============================================================================
// Agent 每日摘要存档
// ============================================================================

/// UPSERT agent 每日 LLM 日志摘要
///
/// 由 Agent 通过 WebSocket DailySummary 消息上报，游戏日结束时生成。
/// Server 注入 created_at 时间戳（服务器权威时间）。
pub async fn upsert_agent_daily_summary(
    pool: &PgPool,
    agent_id: uuid::Uuid,
    game_day: i64,
    summary: &str,
    created_at: i64,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO agent_daily_summaries (agent_id, game_day, summary, created_at)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (agent_id, game_day) DO UPDATE SET
            summary = EXCLUDED.summary,
            created_at = EXCLUDED.created_at
        "#,
    )
    .bind(agent_id)
    .bind(game_day)
    .bind(summary)
    .bind(created_at)
    .execute(pool)
    .await
    .context("UPSERT agent_daily_summary 失败")?;

    debug!(
        "agent_daily_summaries upserted: agent_id={}, game_day={}",
        agent_id, game_day
    );
    Ok(())
}

/// 列出 Agent 每日摘要（支持分页和过滤）
///
/// # 参数
/// - pool: 数据库连接池
/// - agent_id: 可选，按 Agent ID 过滤
/// - game_day: 可选，按游戏日过滤
/// - limit: 返回条数限制，默认 50
/// - offset: 偏移量，默认 0
///
/// # 返回
/// - Ok(Vec<AgentDailySummary>): 符合条件的摘要列表
pub async fn list_agent_daily_summaries(
    pool: &PgPool,
    agent_id: Option<uuid::Uuid>,
    game_day: Option<i64>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<AgentDailySummary>> {
    let limit = limit.unwrap_or(50).min(200);
    let offset = offset.unwrap_or(0);

    let mut query =
        "SELECT id, agent_id, game_day, summary, created_at FROM agent_daily_summaries WHERE 1=1"
            .to_string();
    let mut param_idx = 1;

    if agent_id.is_some() {
        query.push_str(&format!(" AND agent_id = ${}", param_idx));
        param_idx += 1;
    }
    if game_day.is_some() {
        query.push_str(&format!(" AND game_day = ${}", param_idx));
        param_idx += 1;
    }

    query.push_str(&format!(
        " ORDER BY game_day DESC, agent_id ASC LIMIT ${}",
        param_idx
    ));
    param_idx += 1;
    query.push_str(&format!(" OFFSET ${}", param_idx));

    let mut q = sqlx::query_as::<_, AgentDailySummary>(&query);
    if let Some(aid) = agent_id {
        q = q.bind(aid);
    }
    if let Some(gd) = game_day {
        q = q.bind(gd);
    }
    q = q.bind(limit).bind(offset);

    q.fetch_all(pool)
        .await
        .context("查询 agent_daily_summaries 失败")
}

/// 获取指定 Agent 的每日摘要列表
pub async fn get_agent_daily_summaries_by_agent(
    pool: &PgPool,
    agent_id: uuid::Uuid,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<AgentDailySummary>> {
    list_agent_daily_summaries(pool, Some(agent_id), None, limit, offset).await
}

/// 统计 Agent 每日摘要总数（支持过滤）
pub async fn count_agent_daily_summaries(
    pool: &PgPool,
    agent_id: Option<uuid::Uuid>,
    game_day: Option<i64>,
) -> Result<i64> {
    let mut query = "SELECT COUNT(*) FROM agent_daily_summaries WHERE 1=1".to_string();
    let mut bind_idx = 1;

    if agent_id.is_some() {
        query.push_str(&format!(" AND agent_id = ${}", bind_idx));
        bind_idx += 1;
    }
    if game_day.is_some() {
        query.push_str(&format!(" AND game_day = ${}", bind_idx));
        // bind_idx incremented but not used further (query construction done)
    }

    let mut q = sqlx::query_scalar::<_, i64>(&query);
    if let Some(aid) = agent_id {
        q = q.bind(aid);
    }
    if let Some(gd) = game_day {
        q = q.bind(gd);
    }

    q.fetch_one(pool)
        .await
        .context("统计 agent_daily_summaries 数量失败")
}

// ============================================================================
// 涌现：跨 tick 动作观察
// ============================================================================

/// 批量获取多个 Agent 的近期动作记录
///
/// 从 agent_action_logs 表中查询指定 Agent 在 `since_tick` 之后的动作，
/// 按 tick 降序排列，每个 Agent 最多返回 `limit_per_agent` 条。
#[allow(clippy::type_complexity)]
pub async fn get_recent_actions_batch(
    pool: &PgPool,
    agent_ids: &[uuid::Uuid],
    since_tick: i64,
    limit_per_agent: usize,
) -> Result<std::collections::HashMap<uuid::Uuid, Vec<cyber_jianghu_protocol::RecentAction>>> {
    if agent_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let agent_id_vec: Vec<uuid::Uuid> = agent_ids.to_vec();

    let rows: Vec<(uuid::Uuid, i64, String, Option<String>, Option<String>)> =
        sqlx::query_as::<Postgres, (uuid::Uuid, i64, String, Option<String>, Option<String>)>(
            "SELECT agent_id, tick_id, action_type,
                    action_data->>'content' as content,
                    result
             FROM agent_action_logs
             WHERE agent_id = ANY($1) AND tick_id >= $2
             ORDER BY agent_id, tick_id DESC",
        )
        .bind(&agent_id_vec)
        .bind(since_tick)
        .fetch_all(pool)
        .await
        .context("批量获取近期动作失败")?;

    let mut map: std::collections::HashMap<uuid::Uuid, Vec<cyber_jianghu_protocol::RecentAction>> =
        std::collections::HashMap::new();

    for (agent_id, tick_id, action_type, content, result) in rows {
        let actions = map.entry(agent_id).or_default();
        if actions.len() < limit_per_agent {
            actions.push(cyber_jianghu_protocol::RecentAction {
                tick_id,
                action_type,
                content,
                result: result.unwrap_or_else(|| "unknown".to_string()),
            });
        }
    }

    Ok(map)
}

/// Agent 每日动作统计（用于 Server → Agent 推送）
pub struct AgentDailyActionStats {
    pub game_day: i64,
    pub action_counts: std::collections::HashMap<String, i32>,
    pub location_history: Vec<String>,
    pub success_count: i32,
    pub failure_count: i32,
    pub total_actions: i32,
}

/// 查询指定 Agent 在指定游戏日的动作统计
///
/// 将 tick_id 范围映射到 game_day：
///   tick_start = (game_day - 1) * ticks_per_day + 1
///   tick_end = game_day * ticks_per_day
/// 其中 ticks_per_day = ticks_per_hour * hours_per_day = 1 * 12 = 12（配置值）
///
/// 注意：本函数假设所有 tick_id 从 1 开始递增。
pub async fn get_agent_daily_action_stats(
    pool: &PgPool,
    agent_id: uuid::Uuid,
    game_day: i64,
) -> Result<Option<AgentDailyActionStats>> {
    // 从 game_rules 获取 ticks_per_day 配置
    let ticks_per_day = crate::game_data::registry::TimeRegistry::get_config()
        .map(|c| c.ticks_per_hour as i64 * c.hours_per_day as i64)
        .unwrap_or(12); // 降级默认值

    let tick_start = (game_day - 1) * ticks_per_day + 1;
    let tick_end = game_day * ticks_per_day;

    // 动作类型统计
    let count_rows = sqlx::query(
        r#"
        SELECT action_type, COUNT(*) as cnt
        FROM agent_action_logs
        WHERE agent_id = $1 AND tick_id BETWEEN $2 AND $3
        GROUP BY action_type
        "#,
    )
    .bind(agent_id)
    .bind(tick_start)
    .bind(tick_end)
    .fetch_all(pool)
    .await
    .context("查询动作类型统计失败")?;

    if count_rows.is_empty() {
        return Ok(None);
    }

    let mut action_counts = std::collections::HashMap::new();
    let mut total_actions = 0i32;
    for row in &count_rows {
        let action_type: String = row.get("action_type");
        let cnt: i64 = row.get("cnt");
        total_actions += cnt as i32;
        action_counts.insert(action_type, cnt as i32);
    }

    // 成功/失败统计
    let success_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_action_logs WHERE agent_id = $1 AND tick_id BETWEEN $2 AND $3 AND result = 'success'",
    )
    .bind(agent_id)
    .bind(tick_start)
    .bind(tick_end)
    .fetch_one(pool)
    .await
    .context("查询成功动作数失败")?;

    let failure_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_action_logs WHERE agent_id = $1 AND tick_id BETWEEN $2 AND $3 AND result = 'failure'",
    )
    .bind(agent_id)
    .bind(tick_start)
    .bind(tick_end)
    .fetch_one(pool)
    .await
    .context("查询失败动作数失败")?;

    // 地点变化历史（从 agent_states 获取）
    let location_rows = sqlx::query(
        r#"
        SELECT node_id FROM agent_states
        WHERE agent_id = $1 AND tick_id BETWEEN $2 AND $3
        ORDER BY tick_id ASC
        "#,
    )
    .bind(agent_id)
    .bind(tick_start)
    .bind(tick_end)
    .fetch_all(pool)
    .await
    .context("查询地点历史失败")?;

    let location_history: Vec<String> = location_rows
        .into_iter()
        .map(|row| row.get::<String, _>("node_id"))
        .collect();

    Ok(Some(AgentDailyActionStats {
        game_day,
        action_counts,
        location_history,
        success_count: success_count as i32,
        failure_count: failure_count as i32,
        total_actions,
    }))
}
