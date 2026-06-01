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
use std::collections::HashMap;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::models::{Agent, AgentState};

use super::common::generate_secure_token;

// ============================================================================
// 设备连接（Phase 3）
// ============================================================================

/// 设备连接结果
#[derive(Debug)]
pub struct DeviceConnectResult {
    /// 设备 ID
    pub device_id: Uuid,
    /// 认证令牌
    pub auth_token: String,
    /// 是否为新设备
    pub is_new: bool,
}

/// 注册或获取设备
///
/// - 如果设备不存在，创建新设备记录并生成 auth_token
/// - 如果设备已存在，返回现有的 auth_token
///
/// # 参数
/// - pool: 数据库连接池
/// - device_id: 客户端生成的设备 UUID
///
/// # 返回
/// - Ok(DeviceConnectResult): 连接结果
/// - Err: 数据库操作失败
pub async fn connect_device(pool: &PgPool, device_id: Uuid) -> Result<DeviceConnectResult> {
    debug!("设备连接: {}", device_id);

    // 先尝试获取现有设备
    let existing: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT auth_token FROM devices WHERE device_id = $1
        "#,
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await
    .context("查询设备失败")?;

    if let Some((auth_token,)) = existing {
        debug!("设备已存在: {}", device_id);
        return Ok(DeviceConnectResult {
            device_id,
            auth_token,
            is_new: false,
        });
    }

    // 创建新设备
    let auth_token = generate_secure_token();

    sqlx::query(
        r#"
        INSERT INTO devices (device_id, auth_token)
        VALUES ($1, $2)
        ON CONFLICT (device_id) DO UPDATE SET last_seen = CURRENT_TIMESTAMP
        "#,
    )
    .bind(device_id)
    .bind(&auth_token)
    .execute(pool)
    .await
    .context("创建设备记录失败")?;

    tracing::info!("新设备注册成功: {}", device_id);

    Ok(DeviceConnectResult {
        device_id,
        auth_token,
        is_new: true,
    })
}

/// 仅查询设备当前 auth_token（SELECT only，无副作用）
///
/// 与 `connect_device` 的根本区别：
/// - 本函数**永远不修改数据库**，调用方必须先通过 `verify_device_strict`
///   确认设备存在后才能调用，否则会得到 `Ok(None)`
/// - `connect_device` 在设备不存在时会自动 INSERT，是 upsert 语义
///
/// 用于 `device_verify` 端点的 200 路径。**绝不**用于任何需要"创建/复活"
/// 设备的场景——那是 `register_device` 的责任。
///
/// # 参数
/// - pool: 数据库连接池
/// - device_id: 设备 UUID
///
/// # 返回
/// - Ok(Some(token)): 设备存在，返回当前 auth_token
/// - Ok(None): 设备不存在
/// - Err: 数据库查询失败
pub async fn get_device_token(pool: &PgPool, device_id: Uuid) -> Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT auth_token FROM devices WHERE device_id = $1")
            .bind(device_id)
            .fetch_optional(pool)
            .await
            .context("查询设备 token 失败")?;
    Ok(row.map(|(t,)| t))
}

/// 严格校验设备是否存在（仅查询，不创建）
///
/// 与 `connect_device` 的根本区别：本函数**不会**因为设备不存在而自动创建。
/// 用于 agent 启动时验证 device.yaml 中的 device_id 仍被 server 认可。
///
/// # 参数
/// - pool: 数据库连接池
/// - device_id: 设备 UUID
///
/// # 返回
/// - Ok(true): 设备存在
/// - Ok(false): 设备不存在
/// - Err: 数据库错误
pub async fn verify_device_strict(pool: &PgPool, device_id: Uuid) -> Result<bool> {
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT device_id FROM devices WHERE device_id = $1")
        .bind(device_id)
        .fetch_optional(pool)
        .await
        .context("严格校验设备失败")?;
    Ok(row.is_some())
}

/// 显式注册新设备（server 生成 device_id + auth_token）
///
/// 与 `connect_device` 的根本区别：调用者**不能**指定 device_id，必须由 server 生成。
/// 这样从协议层面消除"client 携带任意 UUID 撞库"的可能。
///
/// # 参数
/// - pool: 数据库连接池
///
/// # 返回
/// - Ok(DeviceConnectResult): 包含新 device_id + auth_token，is_new 恒为 true
/// - Err: 数据库错误
pub async fn register_device(pool: &PgPool) -> Result<DeviceConnectResult> {
    let device_id = Uuid::new_v4();
    let auth_token = generate_secure_token();

    sqlx::query(
        r#"
        INSERT INTO devices (device_id, auth_token)
        VALUES ($1, $2)
        "#,
    )
    .bind(device_id)
    .bind(&auth_token)
    .execute(pool)
    .await
    .context("显式注册新设备失败")?;

    info!("新设备显式注册成功: {}", device_id);

    Ok(DeviceConnectResult {
        device_id,
        auth_token,
        is_new: true,
    })
}

/// 验证设备认证令牌
///
/// # 参数
/// - pool: 数据库连接池
/// - device_id: 设备 UUID
/// - auth_token: 认证令牌
///
/// # 返回
/// - Ok(true): 验证通过
/// - Ok(false): 验证失败
/// - Err: 数据库错误
pub async fn verify_device_token(pool: &PgPool, device_id: Uuid, auth_token: &str) -> Result<bool> {
    let result: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM devices WHERE device_id = $1 AND auth_token = $2
        "#,
    )
    .bind(device_id)
    .bind(auth_token)
    .fetch_optional(pool)
    .await
    .context("验证设备令牌失败")?;

    Ok(result.is_some())
}

/// 更新设备最后在线时间
pub async fn update_device_last_seen(pool: &PgPool, device_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE devices SET last_seen = CURRENT_TIMESTAMP WHERE device_id = $1
        "#,
    )
    .bind(device_id)
    .execute(pool)
    .await
    .context("更新设备在线时间失败")?;

    Ok(())
}

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
    /// 初始状态（预留：用于返回给调用方验证）
    #[allow(dead_code)]
    pub initial_state: AgentState,
}

/// 事务性注册Agent（F-04）
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
pub async fn register_agent_transactional(
    pool: &PgPool,
    device_id: Uuid,
    name: &str,
    system_prompt: &str,
    initial_tick_id: i64,
    initial_items: &[(String, String, i32, String)],
) -> Result<RegistrationResult> {
    debug!("事务性注册Agent: {} (device: {})", name, device_id);

    // 开始事务
    let mut tx = pool.begin().await.context("开始事务失败")?;

    // 步骤0: 检查是否已有活跃角色
    let active_count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*) FROM agents WHERE device_id = $1 AND status = 'active'
        "#,
    )
    .bind(device_id)
    .fetch_one(&mut *tx)
    .await
    .context("检查活跃角色失败")?;

    if active_count.0 > 0 {
        anyhow::bail!("该设备已有活跃角色，请先归隐当前角色后再创建新角色");
    }

    // 步骤1: 创建Agent（关联设备，默认状态为 active，记录 birth_tick）
    // birth_tick 需偏移 starting_age，使 compute_age_years 返回 starting_age 而非 0
    let starting_age_ticks = crate::tick::decay::compute_starting_age_ticks();
    let birth_tick = initial_tick_id - starting_age_ticks;
    let agent = sqlx::query_as::<Postgres, Agent>(
        r#"
        INSERT INTO agents (device_id, name, system_prompt, status, birth_tick)
        VALUES ($1, $2, $3, 'active', $4)
        RETURNING *
        "#,
    )
    .bind(device_id)
    .bind(name)
    .bind(system_prompt)
    .bind(birth_tick)
    .fetch_one(&mut *tx)
    .await
    .context("在事务中创建 Agent 失败")?;

    let agent_id = agent.agent_id;
    debug!("事务中创建Agent成功: {} ({})", agent.name, agent_id);

    // 步骤2: 创建初始状态
    let initial_state = AgentState::new(agent_id, initial_tick_id);
    let attributes_json = super::state_ops::serialize_attributes_with_skills(&initial_state)
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

// ============================================================================
// Agent 归隐（retire）
// ============================================================================

/// 归隐结果
#[derive(Debug)]
pub struct RetireResult {
    /// 被归隐的 Agent ID（无活跃角色时为 None）
    pub retired_agent_id: Option<Uuid>,
    /// 被归隐的 Agent 名称（无活跃角色时为 None）
    pub retired_name: Option<String>,
    /// 是否执行了归隐操作（false = 角色已是 dead/retired 终态）
    pub action_taken: bool,
}

/// 归隐当前设备的活跃角色
///
/// 幂等操作：如果设备没有活跃角色（已 dead/retired/none），返回成功但 action_taken=false。
/// 如果有活跃角色，标记为 retired 并插入 is_alive=false 快照防止 Tick 处理。
pub async fn retire_agent(
    pool: &PgPool,
    device_id: Uuid,
    auth_token: &str,
) -> Result<RetireResult> {
    debug!("Agent 归隐请求: device_id={}", device_id);

    let valid = verify_device_token(pool, device_id, auth_token).await?;
    if !valid {
        anyhow::bail!("设备认证失败");
    }

    let agent_info: Option<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT a.agent_id, a.name
        FROM agents a
        WHERE a.device_id = $1 AND a.status = 'active'
        ORDER BY a.created_at DESC
        LIMIT 1
        "#,
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await
    .context("查询 Agent 失败")?;

    let (agent_id, name) = match agent_info {
        Some(info) => info,
        None => {
            info!("设备无活跃角色（已 dead/retired/none），无需归隐");
            return Ok(RetireResult {
                retired_agent_id: None,
                retired_name: None,
                action_taken: false,
            });
        }
    };

    // 3. 标记 Agent 为归隐状态（保留历史数据）
    let updated = sqlx::query(
        r#"
        UPDATE agents
        SET status = 'retired', retired_at = CURRENT_TIMESTAMP
        WHERE agent_id = $1 AND device_id = $2 AND status = 'active'
        "#,
    )
    .bind(agent_id)
    .bind(device_id)
    .execute(pool)
    .await
    .context("更新 Agent 状态失败")?;

    if updated.rows_affected() == 0 {
        anyhow::bail!("归隐失败：角色状态已变更");
    }

    // 4. 插入 is_alive=false 的状态快照，防止归隐角色继续参与 Tick 处理
    // load_agent_states 先 DISTINCT ON 取最新记录再过滤 is_alive，确保最新记录为 false 即可排除
    let latest_tick: Option<i64> =
        sqlx::query_scalar("SELECT MAX(tick_id) FROM agent_states WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_optional(pool)
            .await
            .context("查询 Agent 最新 tick_id 失败")?
            .flatten();

    // 使用下一个 tick_id 避免违反 UNIQUE(agent_id, tick_id) 约束
    let retired_tick_id = latest_tick.map(|t| t + 1).unwrap_or(0);

    sqlx::query(
        r#"
        INSERT INTO agent_states (agent_id, tick_id, attributes, node_id, is_alive)
        VALUES ($1, $2, '{}'::jsonb, 'void', false)
        "#,
    )
    .bind(agent_id)
    .bind(retired_tick_id)
    .execute(pool)
    .await
    .context("插入归隐状态快照失败")?;

    tracing::info!(
        "Agent 归隐成功: {} ({}) 已归隐，可创建新角色",
        name,
        agent_id
    );

    Ok(RetireResult {
        retired_agent_id: Some(agent_id),
        retired_name: Some(name),
        action_taken: true,
    })
}

// ============================================================================
// 自动重生（转世：dead → retired + 创建全新 agent）
// ============================================================================

/// 自动重生结果
pub struct AutoRebirthResult {
    /// 新 Agent ID（全新 UUID）
    pub agent_id: Uuid,
    /// 角色名称
    pub name: String,
    /// 重生位置
    pub spawn_location: String,
}

/// 自动转世重生：旧 agent 保持 status='dead' 死亡标记，INSERT 全新 agent
///
/// 用户硬性约束：不允许将已死亡角色设置为归隐（status='retired'）。
/// `retired` 状态语义专属"玩家主动归隐"（通过 /api/v1/agent/retire 触发）。
///
/// 转世完成后：
/// - 旧 agent 保持 `status='dead'` 死亡标记
/// - `retired_at` 字段作为时间戳记录"转世完成"事件（用于区分"未转世的死角色"和"已转世的死角色"）
/// - retired 状态完全不被 auto-rebirth 触及
///
/// 事务内完成：
/// 1. 查询旧 agent（确认 dead 状态 + 获取基础信息）
/// 2. 旧 agent 仅写 `retired_at` 时间戳，status 保持 'dead'
/// 3. INSERT 新 agent（新 UUID，同 device_id/name/system_prompt）
/// 4. INSERT agent_states（初始属性）
/// 5. INSERT agent_inventory（初始物品）
///
/// 调用者负责更新 DashMap 和 agent_to_device_map。
pub async fn auto_rebirth_agent(
    pool: &PgPool,
    old_agent_id: Uuid,
    spawn_location: &str,
    initial_items: &[(String, String, i32, String)],
    starting_age_ticks: i64,
    reset_recipes: bool,
) -> Result<AutoRebirthResult> {
    debug!(
        "自动转世重生: old_agent={}, spawn={}",
        old_agent_id, spawn_location
    );

    // 开始事务
    let mut tx = pool.begin().await.context("开始转世事务失败")?;

    // 1. 查询旧 agent（确认 dead 状态 + 获取基础信息）
    let old_agent: Option<(String, String, Uuid)> = sqlx::query_as(
        r#"
        SELECT name, system_prompt, device_id FROM agents WHERE agent_id = $1 AND status = 'dead'
        "#,
    )
    .bind(old_agent_id)
    .fetch_optional(&mut *tx)
    .await
    .context("查询旧 Agent 失败")?;

    let (name, system_prompt, device_id) = match old_agent {
        Some(a) => a,
        None => anyhow::bail!("Agent {} 不存在或非 dead 状态，无法转世", old_agent_id),
    };

    // 2. 旧 agent 保持 status='dead' 死亡标记
    //    retired_at 作为时间戳记录"转世完成"事件
    //    严禁写 status='retired'（用户硬性约束：不允许将已死亡角色设置为归隐）
    let update_result = sqlx::query(
        "UPDATE agents SET retired_at = NOW() \
         WHERE agent_id = $1 AND status = 'dead'",
    )
    .bind(old_agent_id)
    .execute(&mut *tx)
    .await
    .context("记录旧 Agent 转世时间戳失败")?;

    if update_result.rows_affected() == 0 {
        anyhow::bail!(
            "Agent {} 转世中止：UPDATE 未影响任何行（可能并发状态变更）",
            old_agent_id
        );
    }
    debug!(
        "旧 Agent {} 保持 status='dead' 死亡标记，retired_at 已记录转世时刻",
        old_agent_id
    );

    // 3. 获取当前 tick_id（用于 birth_tick 计算）
    let current_tick: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(tick_id), 0) FROM agent_states WHERE agent_id = $1",
    )
    .bind(old_agent_id)
    .fetch_one(&mut *tx)
    .await
    .context("查询 tick_id 失败")?;

    let state_tick = current_tick + 1;
    let birth_tick = state_tick - starting_age_ticks;

    // 4. INSERT 新 agent（新 UUID 由 DB 自动生成）
    let new_agent_id: (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO agents (device_id, name, system_prompt, status, birth_tick)
        VALUES ($1, $2, $3, 'active', $4)
        RETURNING agent_id
        "#,
    )
    .bind(device_id)
    .bind(&name)
    .bind(&system_prompt)
    .bind(birth_tick)
    .fetch_one(&mut *tx)
    .await
    .context("创建新 Agent 失败")?;

    let new_agent_id = new_agent_id.0;

    // 5. INSERT agent_states（初始属性）
    let initial_state = crate::models::AgentState::new(new_agent_id, state_tick);
    let attrs = super::state_ops::serialize_attributes_with_skills(&initial_state)
        .context("序列化初始属性失败")?;

    sqlx::query(
        r#"
        INSERT INTO agent_states (agent_id, tick_id, attributes, node_id, is_alive)
        VALUES ($1, $2, $3, $4, true)
        "#,
    )
    .bind(new_agent_id)
    .bind(state_tick)
    .bind(attrs)
    .bind(spawn_location)
    .execute(&mut *tx)
    .await
    .context("插入新 Agent 初始状态失败")?;

    // 6. INSERT agent_inventory（初始物品）
    for item in initial_items {
        sqlx::query(
            r#"
            INSERT INTO agent_inventory (agent_id, item_id, quantity)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(new_agent_id)
        .bind(&item.0)
        .bind(item.2)
        .execute(&mut *tx)
        .await
        .context("分配初始物品失败")?;
    }

    // 6. 重生配方重置（事务内，配置驱动）
    if reset_recipes {
        sqlx::query("DELETE FROM agent_known_recipes WHERE agent_id = $1")
            .bind(old_agent_id)
            .execute(&mut *tx)
            .await
            .context("重置旧配方失败")?;
        sqlx::query("DELETE FROM agent_recipe_observations WHERE agent_id = $1")
            .bind(old_agent_id)
            .execute(&mut *tx)
            .await
            .context("重置旧观察记录失败")?;
    }

    // 提交事务
    tx.commit().await.context("提交转世事务失败")?;

    info!(
        "Agent 转世重生成功: {} ({} → {}) → {}",
        name, old_agent_id, new_agent_id, spawn_location
    );

    Ok(AutoRebirthResult {
        agent_id: new_agent_id,
        name,
        spawn_location: spawn_location.to_string(),
    })
}

// ============================================================================
// 配方知识 CRUD
// ============================================================================

/// 批量分配 Agent 初始配方
pub async fn assign_initial_recipes(
    pool: &PgPool,
    agent_id: Uuid,
    recipe_ids: &[String],
    tick_id: i64,
) -> Result<()> {
    for recipe_id in recipe_ids {
        sqlx::query(
            "INSERT INTO agent_known_recipes (agent_id, recipe_id, learned_at_tick, source)
             VALUES ($1, $2, $3, 'initial')
             ON CONFLICT (agent_id, recipe_id) DO NOTHING",
        )
        .bind(agent_id)
        .bind(recipe_id)
        .bind(tick_id)
        .execute(pool)
        .await
        .context("分配初始配方失败")?;
    }
    Ok(())
}

/// 查询 Agent 已知配方 ID 列表
pub async fn get_known_recipe_ids(pool: &PgPool, agent_id: Uuid) -> Result<Vec<String>> {
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT recipe_id FROM agent_known_recipes WHERE agent_id = $1",
    )
    .bind(agent_id)
    .fetch_all(pool)
    .await
    .context("查询已知配方失败")?;

    Ok(rows)
}

/// 批量查询多个 Agent 的已知配方 ID
pub async fn batch_get_known_recipe_ids(
    pool: &PgPool,
    agent_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<String>>> {
    if agent_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT agent_id, recipe_id FROM agent_known_recipes WHERE agent_id = ANY($1)",
    )
    .bind(agent_ids)
    .fetch_all(pool)
    .await
    .context("批量查询已知配方失败")?;

    let mut map: HashMap<Uuid, Vec<String>> = HashMap::new();
    for (agent_id, recipe_id) in rows {
        map.entry(agent_id).or_default().push(recipe_id);
    }
    Ok(map)
}

/// 记录配方观察，返回观察计数
pub async fn record_recipe_observation(
    pool: &PgPool,
    observer_id: Uuid,
    recipe_id: &str,
    tick_id: i64,
) -> Result<i32> {
    let existing: Option<(i32,)> =
        sqlx::query_as("SELECT observation_count FROM agent_recipe_observations WHERE agent_id = $1 AND recipe_id = $2")
            .bind(observer_id)
            .bind(recipe_id)
            .fetch_optional(pool)
            .await
            .context("查询观察计数失败")?;

    let count = match existing {
        Some((c,)) => {
            sqlx::query(
                "UPDATE agent_recipe_observations SET observation_count = $3, last_seen_tick = $4
                 WHERE agent_id = $1 AND recipe_id = $2",
            )
            .bind(observer_id)
            .bind(recipe_id)
            .bind(c + 1)
            .bind(tick_id)
            .execute(pool)
            .await
            .context("更新观察计数失败")?;
            c + 1
        }
        None => {
            sqlx::query(
                "INSERT INTO agent_recipe_observations (agent_id, recipe_id, observation_count, last_seen_tick)
                 VALUES ($1, $2, 1, $3)",
            )
            .bind(observer_id)
            .bind(recipe_id)
            .bind(tick_id)
            .execute(pool)
            .await
            .context("插入观察记录失败")?;
            1
        }
    };

    Ok(count)
}
