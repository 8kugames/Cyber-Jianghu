// ============================================================================
// 生命周期：归隐与自动重生（dead → retired + 新 agent）
// ============================================================================

use super::*;

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
    let latest_tick: Option<i64> = sqlx::query_scalar!(
        "SELECT MAX(tick_id) FROM agent_states WHERE agent_id = $1",
        agent_id,
    )
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

    // 归隐=旧凭据失效。立即轮换 device.auth_token，
    // 防止同设备连续创建角色时，旧凭据仍可被复用攻击新角色。
    if let Err(e) = rotate_device_token(pool, device_id).await {
        // 归隐已成功，仅记 error，不阻断主流程。
        // 客户端下次 connect_device 时会拿到新 token（重试或重新注册可恢复）。
        tracing::error!(
            "retire_agent 后轮换 device token 失败: device={}, err={}",
            device_id,
            e
        );
    }

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
    /// 服务端权威 system_prompt
    pub system_prompt: String,
    /// 重生位置
    pub spawn_location: String,
}

/// 自动转世重生参数（打包 spawn_location 等 5 个业务参数，避免函数签名超过 7 个参数）
#[derive(Debug, Clone)]
pub struct AutoRebirthParams<'a> {
    /// 重生位置
    pub spawn_location: &'a str,
    /// 初始物品 [(item_id, name, quantity, description)]
    pub initial_items: &'a [(String, String, i32, String)],
    /// 起始年龄（tick 数）
    pub starting_age_ticks: i64,
    /// 是否重置配方
    pub reset_recipes: bool,
    /// 当前世界 tick
    pub world_tick: i64,
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
    device_id: Uuid,
    params: AutoRebirthParams<'_>,
) -> Result<AutoRebirthResult> {
    let AutoRebirthParams {
        spawn_location,
        initial_items,
        starting_age_ticks,
        reset_recipes,
        world_tick,
    } = params;

    debug!(
        "自动转世重生: old_agent={}, spawn={}",
        old_agent_id, spawn_location
    );

    // 开始事务
    let mut tx = pool.begin().await.context("开始转世事务失败")?;

    // 1. 查询旧 agent（必须 device_id 匹配，杜绝跨设备转世）
    let old_agent: Option<(String, String, Uuid, Option<String>)> =
        sqlx::query_as(REBIRTH_FETCH_OLD_AGENT_SQL)
            .bind(old_agent_id)
            .bind(device_id)
            .fetch_optional(&mut *tx)
            .await
            .context("查询旧 Agent 失败")?;

    let (name, system_prompt, fetched_device_id, inherited_model_id) = match old_agent {
        Some(a) => a,
        None => anyhow::bail!(
            "Agent {} 不存在、不属于设备 {} 或非 dead 状态，无法转世",
            old_agent_id,
            device_id
        ),
    };

    // 双重保险：fetch 已按 device_id 过滤，但此处再断言一次
    // 防止未来重构中 SQL 静默移除 device_id 条件。
    debug_assert_eq!(fetched_device_id, device_id);

    // 2. 旧 agent 保持 status='dead' 死亡标记
    //    retired_at 作为时间戳记录"转世完成"事件
    //    严禁写 status='retired'（用户硬性约束：不允许将已死亡角色设置为归隐）
    // AND retired_at IS NULL 守卫，阻断 agent 端 retry 触发的重复重生。
    let update_result = sqlx::query(REBIRTH_MARK_RETIRED_SQL)
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

    // 3. 用 caller 传入的世界 tick（`state.current_accepting_tick_id`
    //    优先，回退到 `get_current_world_tick_id`）推导 state_tick / birth_tick。
    //    旧实现 `MAX(agent_states.tick_id) WHERE agent_id = old + 1` 会让新角色
    //    state 落后世界 N tick，导致 birth_tick 偏小、compute_age_years 异常。
    let (state_tick, birth_tick) = compute_rebirth_ticks(world_tick, starting_age_ticks);

    // 4. INSERT 新 agent（新 UUID 由 DB 自动生成），继承旧角色的 model_id
    let new_agent_id: (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO agents (device_id, name, system_prompt, status, birth_tick, model_id)
        VALUES ($1, $2, $3, 'active', $4, $5)
        RETURNING agent_id
        "#,
    )
    .bind(device_id)
    .bind(&name)
    .bind(&system_prompt)
    .bind(birth_tick)
    .bind(&inherited_model_id)
    .fetch_one(&mut *tx)
    .await
    .context("创建新 Agent 失败")?;

    let new_agent_id = new_agent_id.0;

    // 5. INSERT agent_states（初始属性）
    let initial_state = crate::models::AgentState::new(new_agent_id, state_tick);
    let attrs = super::super::state_ops::serialize_attributes_with_skills(&initial_state)
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
        system_prompt,
        spawn_location: spawn_location.to_string(),
    })
}
