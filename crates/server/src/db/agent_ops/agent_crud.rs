// ============================================================================
// Agent CRUD（查询/在线/位置/传记/超时统计/事务注册）
// ============================================================================

use super::*;

// ============================================================================
// Agent 相关操作
// ============================================================================

/// 根据agent_id查询Agent
///
/// # 参数
/// - pool: 数据库连接池
/// - agent_id: Agent ID
///
/// # 返回
/// - Ok(Agent): 查询到的Agent
/// - Err: 查询失败或未找到
pub async fn get_agent_by_id(pool: &PgPool, agent_id: Uuid) -> Result<Agent> {
    debug!("查询Agent by id: {}", agent_id);

    let agent = sqlx::query_as::<Postgres, Agent>(
        r#"
        SELECT * FROM agents WHERE agent_id = $1
        "#,
    )
    .bind(agent_id)
    .fetch_one(pool)
    .await
    .context("根据 agent_id 查询 Agent 失败")?;

    Ok(agent)
}

/// 根据设备ID获取Agent（优先返回活跃，其次返回已死亡）
///
/// 返回该设备最新的、非归隐状态的 Agent：
/// - `active`：正常返回
/// - `dead`：返回（让 agent 知道自己已死亡，而非"未注册"）
/// - `retired`：不返回（用户主动注销，等同未注册）
///
/// # 参数
/// - pool: 数据库连接池
/// - device_id: 设备ID
///
/// # 返回
/// - Ok(Some(Agent)): 找到活跃或已死亡的 Agent
/// - Ok(None): 无 Agent 或已归隐
/// - Err: 查询失败
pub async fn get_agent_by_device_id(pool: &PgPool, device_id: Uuid) -> Result<Option<Agent>> {
    debug!("查询Agent by device_id: {}", device_id);

    // 优先活跃，其次死亡（按创建时间倒序取最新）
    let agent = sqlx::query_as::<Postgres, Agent>(
        r#"
        SELECT * FROM agents
        WHERE device_id = $1 AND status IN ('active', 'dead')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await
    .context("根据 device_id 查询 Agent 失败")?;

    Ok(agent)
}

/// 获取所有Agent
///
/// # 参数
/// - pool: 数据库连接池
///
/// # 返回
/// - `Ok(Vec<Agent>)`: 所有Agent列表
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
pub async fn update_agent_location(
    conn: &mut sqlx::PgConnection,
    agent_id: Uuid,
    node_id: &str,
) -> Result<()> {
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
    .execute(&mut *conn)
    .await
    .context("更新 Agent 位置失败")?;

    Ok(())
}

/// 更新 Agent 传记（纪传体）
pub async fn update_agent_biography(pool: &PgPool, agent_id: Uuid, biography: &str) -> Result<()> {
    sqlx::query("UPDATE agents SET biography = $1 WHERE agent_id = $2")
        .bind(biography)
        .bind(agent_id)
        .execute(pool)
        .await
        .context("更新 Agent 传记失败")?;
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
    let total_alive_agents: i64 = sqlx::query_scalar!(
        r#"
        SELECT COUNT(DISTINCT s.agent_id) as "count!"
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
    .context("获取存活 Agent 总数失败")?;

    // 查询超时Agent数量（30秒内未上报意图）
    let timeout_agents: i64 = sqlx::query_scalar!(
        r#"
        SELECT COUNT(DISTINCT s.agent_id) as "count!"
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
        timeout_window_secs as f64 / 60.0, // 转换为分钟
    )
    .fetch_one(pool)
    .await
    .context("获取超时 Agent 数量失败")?;

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
    /// 初始状态（预留：用于返回给调用方验证）
    #[allow(dead_code)]
    pub initial_state: AgentState,
}

/// 事务性注册Agent
///
/// 在单个数据库事务中执行：
/// 1. 创建Agent记录（关联到设备）
/// 2. 创建初始状态
/// 3. 分配初始物品
///
/// 任何步骤失败都会回滚整个事务
///
/// # 参数
/// - pool: 数据库连接池
/// - device_id: 设备ID（Agent所属设备）
/// - name: Agent名称
/// - system_prompt: Agent人设Prompt
/// - initial_tick_id: 初始Tick ID
/// - initial_items: 初始物品列表
/// - model_id: 角色注册时上报的 LLM 模型 ID（可选）
pub async fn register_agent_transactional(
    pool: &PgPool,
    device_id: Uuid,
    name: &str,
    system_prompt: &str,
    initial_tick_id: i64,
    initial_items: &[(String, String, i32, String)],
    model_id: Option<&str>,
) -> Result<RegistrationResult> {
    debug!("事务性注册Agent: {} (device: {})", name, device_id);

    // 开始事务
    let mut tx = pool.begin().await.context("开始事务失败")?;

    // 步骤0: 检查是否已有活跃角色
    let active_count: i64 = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!" FROM agents WHERE device_id = $1 AND status = 'active'
        "#,
        device_id,
    )
    .fetch_one(&mut *tx)
    .await
    .context("检查活跃角色失败")?;

    if active_count > 0 {
        anyhow::bail!("该设备已有活跃角色，请先归隐当前角色后再创建新角色");
    }

    // 步骤1: 创建Agent（关联设备，默认状态为 active，记录 birth_tick）
    // birth_tick 需偏移 starting_age，使 compute_age_years 返回 starting_age 而非 0
    let starting_age_ticks = crate::tick::decay::compute_starting_age_ticks();
    let birth_tick = initial_tick_id - starting_age_ticks;
    let agent = sqlx::query_as::<Postgres, Agent>(
        r#"
        INSERT INTO agents (device_id, name, system_prompt, status, birth_tick, model_id)
        VALUES ($1, $2, $3, 'active', $4, $5)
        RETURNING *
        "#,
    )
    .bind(device_id)
    .bind(name)
    .bind(system_prompt)
    .bind(birth_tick)
    .bind(model_id)
    .fetch_one(&mut *tx)
    .await
    .context("在事务中创建 Agent 失败")?;

    let agent_id = agent.agent_id;
    debug!("事务中创建Agent成功: {} ({})", agent.name, agent_id);

    // 步骤2: 创建初始状态
    let initial_state = AgentState::new(agent_id, initial_tick_id);
    let attributes_json = super::super::state_ops::serialize_attributes_with_skills(&initial_state)
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

    debug!(
        "事务中创建初始状态成功: agent={}, tick={}",
        agent_id, initial_tick_id
    );

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

    // 验证：查询实际插入的物品数量
    let check: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_inventory WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&mut *tx)
        .await
        .context("验证初始物品插入失败")?;

    if check.0 != initial_items.len() as i64 {
        error!(
            "初始物品数量不匹配！预期: {}, 实际: {}",
            initial_items.len(),
            check.0
        );
        // 注意：不强制失败，因为可能是有意为之（如配置为空）
    } else {
        info!("初始物品验证通过: {} 件", check.0);
    }

    // 提交事务
    tx.commit().await.context("提交注册事务失败")?;

    tracing::info!("Agent注册事务完成: {} ({})", agent.name, agent_id);

    Ok(RegistrationResult {
        agent,
        initial_state: state,
    })
}
