// ============================================================================
// 配方知识 CRUD（事务内初始配方 / 已知配方查询）
// ============================================================================

use super::*;

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
    let rows: Vec<String> = sqlx::query_scalar!(
        "SELECT recipe_id FROM agent_known_recipes WHERE agent_id = $1",
        agent_id,
    )
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

/// 转世重生的 tick 计算（纯函数，可单测）。
///
/// 之前用 `MAX(agent_states.tick_id) WHERE agent_id = old` 取旧角色
/// 的最后状态 tick，再 +1 当新角色 tick。这套逻辑在"死亡到重生之间世界已推进
/// N tick"时会让新角色 state 落后世界 N tick，进而 `birth_tick` 偏小、
/// `compute_age_years` 返回的年龄小于 `starting_age`、寿终检查 / telemetry
/// 统计全部偏移。
///
/// 正确语义：重生即"现在"。新 agent 状态行的 `state_tick` 直接使用
/// caller 传入的 `world_tick`（由 `state.current_accepting_tick_id` 或
/// `get_current_world_tick_id` 取到），`birth_tick` 由此反推
/// `world_tick - starting_age_ticks`，保证 `compute_age_years(birth_tick, world_tick) == starting_age`。
pub fn compute_rebirth_ticks(world_tick: i64, starting_age_ticks: i64) -> (i64, i64) {
    (world_tick, world_tick - starting_age_ticks)
}

/// 前置拦截 `old_agent_id == Uuid::nil()`，避免无意义 round-trip。
///
/// 旧行为：nil 也走 `auto_rebirth_agent` 内部 WHERE 过滤，DB 报
/// "Agent 00000000-... 不存在或非 dead 状态"，靠副作用防错。Agent 端
/// `death.rs:161-167` 早就在客户端就拦了，server 端必须一致。
pub fn ensure_old_agent_id_not_nil(old_agent_id: Uuid) -> anyhow::Result<()> {
    if old_agent_id.is_nil() {
        anyhow::bail!("old_agent_id 不能为空 UUID");
    }
    Ok(())
}

/// 核心 SQL 集中点。
///
/// - F2：`AND device_id = $2` 强制旧 agent 必须属于 caller 的设备，杜绝跨设备转世。
/// - F3：fetch 不带 retired_at 过滤，但配合下面的 UPDATE 守卫实现幂等性。
pub(crate) const REBIRTH_FETCH_OLD_AGENT_SQL: &str = r#"
SELECT name, system_prompt, device_id, model_id
FROM agents
WHERE agent_id = $1
  AND device_id = $2
  AND status = 'dead'
"#;

/// UPDATE 增加 `AND retired_at IS NULL` 守卫，
/// 阻断 agent 端 retry 触发"同 dead agent 多次转世"。
pub(crate) const REBIRTH_MARK_RETIRED_SQL: &str = r#"
UPDATE agents
SET retired_at = NOW()
WHERE agent_id = $1
  AND status = 'dead'
  AND retired_at IS NULL
"#;

#[cfg(test)]
#[path = "../agent_ops_tests.rs"]
mod tests;
