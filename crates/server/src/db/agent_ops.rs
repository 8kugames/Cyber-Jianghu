// ============================================================================
// OpenClaw Cyber-Jianghu MVP Agent数据库操作模块
// ============================================================================
//
// 本模块实现Agent相关的数据库操作，包括：
// - 创建Agent
// - 查询Agent（by ID, by token, all）
// - 更新Agent状态（在线时间、位置）

use anyhow::{Context, Result};
use sqlx::{PgPool, Postgres, Row};
use tracing::debug;
use uuid::Uuid;

use crate::models::{Agent, AgentState};

use super::common::generate_secure_token;

// ============================================================================
// Agent 相关操作
// ============================================================================

/// 创建新Agent
///
/// # 参数
/// - pool: 数据库连接池
/// - name: Agent名称
/// - system_prompt: Agent人设Prompt
///
/// # 返回
/// - Ok(Agent): 创建的Agent
/// - Err: 创建失败
pub async fn create_agent(pool: &PgPool, name: &str, system_prompt: &str) -> Result<Agent> {
    debug!("创建Agent: {}", name);

    // 生成安全的 auth_token
    // 使用 UUID v4 + 随机后缀，提供 128 位 + 额外64 位
    // 格式: {uuid_v4}_{random_16_hex}
    let auth_token = generate_secure_token();

    let agent = sqlx::query_as::<Postgres, Agent>(
        r#"
        INSERT INTO agents (name, system_prompt, auth_token)
        VALUES ($1, $2, $3)
        RETURNING *
        "#,
    )
    .bind(name)
    .bind(system_prompt)
    .bind(&auth_token)
    .fetch_one(pool)
    .await
    .context("创建 Agent 失败")?;

    tracing::info!("Agent创建成功: {} ({})", agent.name, agent.agent_id);
    Ok(agent)
}

/// 根据auth_token查询Agent
///
/// # 参数
/// - pool: 数据库连接池
/// - auth_token: 认证token
///
/// # 返回
/// - Ok(Agent): 查询到的Agent
/// - Err: 查询失败或未找到
pub async fn get_agent_by_token(pool: &PgPool, auth_token: &str) -> Result<Agent> {
    debug!("查询Agent by token");

    let agent = sqlx::query_as::<Postgres, Agent>(
        r#"
        SELECT * FROM agents WHERE auth_token = $1
        "#,
    )
    .bind(auth_token)
    .fetch_one(pool)
    .await
    .context("根据 token 查询 Agent 失败")?;

    Ok(agent)
}

/// 获取所有Agent
///
/// # 参数
/// - pool: 数据库连接池
///
/// # 返回
/// - Ok(Vec<Agent>): 所有Agent列表
/// - Err: 查询失败
pub async fn get_all_agents(pool: &PgPool) -> Result<Vec<Agent>> {
    debug!("查询所有Agent");

    let agents = sqlx::query_as::<Postgres, Agent>(
        r#"
        SELECT * FROM agents ORDER BY created_at
        "#,
    )
    .fetch_all(pool)
    .await
    .context("获取所有 Agent 列表失败")?;

    debug!("查询到 {} 个Agent", agents.len());
    Ok(agents)
}

/// 更新Agent最后在线时间
///
/// # 参数
/// - pool: 数据库连接池
/// - agent_id: Agent ID
///
/// # 返回
/// - Ok(()): 更新成功
/// - Err: 更新失败
pub async fn update_agent_online(pool: &PgPool, agent_id: Uuid) -> Result<()> {
    debug!("更新Agent在线时间: {}", agent_id);

    sqlx::query(
        r#"
        UPDATE agents
        SET last_tick_online = CURRENT_TIMESTAMP
        WHERE agent_id = $1
        "#,
    )
    .bind(agent_id)
    .execute(pool)
    .await
    .context("更新 Agent 在线时间失败")?;

    Ok(())
}

/// 更新Agent位置
///
/// # 参数
/// - pool: 数据库连接池
/// - agent_id: Agent ID
/// - node_id: 新位置节点ID
///
/// # 返回
/// - Ok(()): 更新成功
/// - Err: 更新失败
pub async fn update_agent_location(pool: &PgPool, agent_id: Uuid, node_id: &str) -> Result<()> {
    debug!("更新Agent位置: {} -> {}", agent_id, node_id);

    sqlx::query(
        r#"
        UPDATE agent_states
        SET node_id = $1
        WHERE agent_id = $2
        AND id = (
            SELECT id FROM agent_states
            WHERE agent_id = $2
            ORDER BY created_at DESC
            LIMIT 1
        )
        "#,
    )
    .bind(node_id)
    .bind(agent_id)
    .execute(pool)
    .await
    .context("更新 Agent 位置失败")?;

    Ok(())
}

/// 意图超时统计
#[derive(Debug, Clone)]
pub struct IntentTimeoutStats {
    /// 总存活 Agent 数量
    pub total_alive_agents: i64,
    /// 超时的 Agent 数量（30秒内未上报意图）
    pub timeout_agents: i64,
    /// 超时率（0-1）
    pub timeout_rate: f64,
}

/// 计算意图超时统计
///
/// 统计在过去30秒内未上报意图的存活Agent数量
///
/// # 参数
/// - pool: 数据库连接池
///
/// # 返回
/// - Ok(IntentTimeoutStats): 超时统计信息
/// - Err: 查询失败
pub async fn get_intent_timeout_stats(pool: &PgPool) -> Result<IntentTimeoutStats> {
    // 30秒时间窗口
    let timeout_window_secs = 30;

    // 查询总存活Agent数量
    let total_alive_agents: i64 = sqlx::query(
        r#"
        SELECT COUNT(DISTINCT s.agent_id) as count
        FROM agent_states s
        INNER JOIN (
            SELECT agent_id, MAX(tick_id) as max_tick
            FROM agent_states
            GROUP BY agent_id
        ) latest ON s.agent_id = latest.agent_id AND s.tick_id = latest.max_tick
        WHERE s.is_alive = true
        "#,
    )
    .fetch_one(pool)
    .await
    .context("获取存活 Agent 总数失败")?
    .get("count");

    // 查询超时Agent数量（30秒内未上报意图）
    let timeout_agents: i64 = sqlx::query(
        r#"
        SELECT COUNT(DISTINCT s.agent_id) as count
        FROM agent_states s
        INNER JOIN (
            SELECT agent_id, MAX(tick_id) as max_tick
            FROM agent_states
            GROUP BY agent_id
        ) latest ON s.agent_id = latest.agent_id AND s.tick_id = latest.max_tick
        LEFT JOIN agents a ON s.agent_id = a.agent_id
        WHERE s.is_alive = true
        AND (
            a.last_tick_online IS NULL
            OR a.last_tick_online < CURRENT_TIMESTAMP - INTERVAL '1 minute' * $1
        )
        "#,
    )
    .bind(timeout_window_secs as f64 / 60.0) // 转换为分钟
    .fetch_one(pool)
    .await
    .context("获取超时 Agent 数量失败")?
    .get("count");

    let timeout_rate = if total_alive_agents > 0 {
        timeout_agents as f64 / total_alive_agents as f64
    } else {
        0.0
    };

    Ok(IntentTimeoutStats {
        total_alive_agents,
        timeout_agents,
        timeout_rate,
    })
}

/// 注册结果
pub struct RegistrationResult {
    pub agent: Agent,
    pub initial_state: AgentState,
}

/// 事务性注册Agent（F-04）
///
/// 在单个数据库事务中执行：
/// 1. 创建Agent记录
/// 2. 创建初始状态
/// 3. 分配初始物品
///
/// 任何步骤失败都会回滚整个事务
pub async fn register_agent_transactional(
    pool: &PgPool,
    name: &str,
    system_prompt: &str,
    initial_tick_id: i64,
    initial_items: &[(String, String, i32, String)],
) -> Result<RegistrationResult> {
    debug!("事务性注册Agent: {}", name);

    let auth_token = generate_secure_token();

    // 开始事务
    let mut tx = pool.begin().await.context("开始事务失败")?;

    // 步骤1: 创建Agent
    let agent = sqlx::query_as::<Postgres, Agent>(
        r#"
        INSERT INTO agents (name, system_prompt, auth_token)
        VALUES ($1, $2, $3)
        RETURNING *
        "#,
    )
    .bind(name)
    .bind(system_prompt)
    .bind(&auth_token)
    .fetch_one(&mut *tx)
    .await
    .context("在事务中创建 Agent 失败")?;

    let agent_id = agent.agent_id;
    debug!("事务中创建Agent成功: {} ({})", agent.name, agent_id);

    // 步骤2: 创建初始状态
    let initial_state = AgentState::new(agent_id, initial_tick_id);
    let attributes_json = serde_json::to_value(&initial_state.get_attributes_for_protocol())
        .context("序列化属性失败")?;

    let state = sqlx::query_as::<Postgres, AgentState>(
        r#"
        INSERT INTO agent_states (agent_id, tick_id, attributes, node_id, is_alive)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING *
        "#,
    )
    .bind(agent_id)
    .bind(initial_tick_id)
    .bind(attributes_json)
    .bind(&initial_state.node_id)
    .bind(initial_state.is_alive)
    .fetch_one(&mut *tx)
    .await
    .context("在事务中创建 Agent 状态失败")?;

    debug!("事务中创建初始状态成功: agent={}, tick={}", agent_id, initial_tick_id);

    // 步骤3: 分配初始物品
    for item in initial_items {
        sqlx::query(
            r#"
            INSERT INTO agent_inventory (agent_id, item_id, quantity)
            VALUES ($1, $2, $3)
            ON CONFLICT (agent_id, item_id)
            DO UPDATE SET
                quantity = agent_inventory.quantity + EXCLUDED.quantity,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(agent_id)
        .bind(&item.0)
        .bind(item.2)
        .execute(&mut *tx)
        .await
        .context("在事务中添加初始物品失败")?;
    }

    debug!("事务中分配初始物品成功: {} 件", initial_items.len());

    // 提交事务
    tx.commit().await.context("提交注册事务失败")?;

    tracing::info!("Agent注册事务完成: {} ({})", agent.name, agent_id);

    Ok(RegistrationResult {
        agent,
        initial_state: state,
    })
}
