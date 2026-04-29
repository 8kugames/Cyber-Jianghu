// ============================================================================
// 数据采集器
// ============================================================================
//
// 从数据库聚合 7 日数据，包括：
// - Agent 基本信息和状态变化
// - 动作日志统计
// - 关键事件提取
// - 地点分布
// ============================================================================

use anyhow::{Context, Result};
use sqlx::Row;
use std::collections::HashMap;

use super::{ActionStats, Highlight, LocationStat};

/// 采集的原始数据
#[derive(Debug, Clone)]
pub struct CollectedData {
    pub period_start: i64,
    pub period_end: i64,
    pub game_day_start: i32,
    pub game_day_end: i32,
    pub season: String,
    pub agents: Vec<AgentInfo>,
    pub highlights: Vec<Highlight>,
    pub action_stats: ActionStats,
    pub location_stats: Vec<LocationStat>,
    pub deaths: i32,
    pub births: i32,
}

/// Agent 信息
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub agent_id: uuid::Uuid,
    pub name: String,
    pub location: String,
    pub actions_count: i32,
    pub top_actions: Vec<(String, i32)>,
    pub narratives: Vec<String>,
    pub died_this_period: bool,
}

/// 采集 7 日数据
pub async fn collect(
    db_pool: &crate::db::DbPool,
    period_start: i64,
    period_end: i64,
) -> Result<CollectedData> {
    let game_days = calculate_game_days(db_pool, period_start, period_end).await?;
    let season = get_season(db_pool, period_end).await?;
    let agents = collect_agents(db_pool, period_start, period_end).await?;
    let highlights = collect_highlights(db_pool, period_start, period_end).await?;
    let (action_stats, location_stats) = collect_stats(db_pool, period_start, period_end).await?;
    let deaths = collect_deaths(db_pool, period_start, period_end).await?;
    let births = collect_births(db_pool, period_start, period_end).await?;

    Ok(CollectedData {
        period_start,
        period_end,
        game_day_start: game_days.0,
        game_day_end: game_days.1,
        season,
        agents,
        highlights,
        action_stats,
        location_stats,
        deaths,
        births,
    })
}

/// 计算游戏日范围
async fn calculate_game_days(
    _db_pool: &crate::db::DbPool,
    period_start: i64,
    period_end: i64,
) -> Result<(i32, i32)> {
    let ticks_per_hour = get_ticks_per_hour().await.unwrap_or(1);

    let start_day = (period_start / (ticks_per_hour * 24)) + 1;
    let end_day = (period_end / (ticks_per_hour * 24)) + 1;

    Ok((start_day as i32, end_day as i32))
}

/// 获取季节
async fn get_season(_db_pool: &crate::db::DbPool, tick_id: i64) -> Result<String> {
    let season = crate::game_data::registry::TimeRegistry::get_current_season(tick_id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "未知".to_string());

    Ok(season)
}

/// 采集 Agent 数据
async fn collect_agents(
    db_pool: &crate::db::DbPool,
    period_start: i64,
    period_end: i64,
) -> Result<Vec<AgentInfo>> {
    // 批量查询：一次获取所有活跃 agent 的基本信息
    let rows = sqlx::query(
        r#"
        SELECT
            a.agent_id,
            a.name,
            COALESCE(
                latest_state.node_id,
                'unknown'
            ) as location
        FROM agents a
        INNER JOIN agent_action_logs l ON a.agent_id = l.agent_id
        LEFT JOIN LATERAL (
            SELECT node_id FROM agent_states s
            WHERE s.agent_id = a.agent_id AND s.tick_id <= $2
            ORDER BY s.tick_id DESC LIMIT 1
        ) latest_state ON true
        WHERE l.tick_id BETWEEN $1 AND $2
        GROUP BY a.agent_id, a.name, latest_state.node_id
        ORDER BY a.name
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询活跃 Agent 失败")?;

    // 批量查询：动作统计（一次性获取所有 agent 的动作数量和类型分布）
    let action_stats_rows = sqlx::query(
        r#"
        SELECT
            agent_id,
            COUNT(*) as actions_count,
            action_type,
            COUNT(*) as type_count
        FROM agent_action_logs
        WHERE tick_id BETWEEN $1 AND $2
        GROUP BY agent_id, action_type
        ORDER BY agent_id, type_count DESC
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询动作统计失败")?;

    // 按 agent_id 分组统计
    let mut agent_actions: std::collections::HashMap<uuid::Uuid, (i64, Vec<(String, i64)>)> =
        std::collections::HashMap::new();

    for row in action_stats_rows {
        let agent_id: uuid::Uuid = row.get("agent_id");
        // COUNT(*) returns BIGINT in PostgreSQL, use i64
        let actions_count: i64 = row.get("actions_count");
        let action_type: String = row.get("action_type");
        let type_count: i64 = row.get("type_count");

        agent_actions
            .entry(agent_id)
            .and_modify(|(cnt, types)| {
                *cnt = actions_count;
                if types.len() < 5 {
                    types.push((action_type.clone(), type_count));
                }
            })
            .or_insert_with(|| (actions_count, vec![(action_type, type_count)]));
    }

    // 批量查询：叙事描述
    let narrative_rows = sqlx::query(
        r#"
        SELECT DISTINCT ON (agent_id) agent_id, narrative
        FROM agent_action_logs
        WHERE tick_id BETWEEN $1 AND $2
        AND narrative IS NOT NULL
        ORDER BY agent_id, tick_id
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询叙事描述失败")?;

    let narratives_map: std::collections::HashMap<uuid::Uuid, String> = narrative_rows
        .into_iter()
        .map(|row| {
            let agent_id: uuid::Uuid = row.get("agent_id");
            let narrative: String = row.get("narrative");
            (agent_id, narrative)
        })
        .collect();

    // 批量查询：每日 LLM 日志摘要（agent_daily_summaries）
    // LEFT JOIN，agent 在本周期无摘要时不影响主查询
    let ticks_per_hour = get_ticks_per_hour().await.unwrap_or(1);
    let period_game_days = (
        (period_start / (ticks_per_hour * 24)) as i64,
        (period_end / (ticks_per_hour * 24)) as i64,
    );

    let daily_summary_rows = sqlx::query(
        r#"
        SELECT ads.agent_id, ads.game_day, ads.summary
        FROM agent_daily_summaries ads
        INNER JOIN agents a ON ads.agent_id = a.agent_id
        WHERE ads.game_day BETWEEN $1 AND $2
        ORDER BY ads.agent_id, ads.game_day ASC
        "#,
    )
    .bind(period_game_days.0)
    .bind(period_game_days.1)
    .fetch_all(db_pool)
    .await
    .context("查询每日摘要失败")?;

    // 按 agent_id 分组，所有 game_day 摘要拼接为一个字符串
    let daily_summaries_map: std::collections::HashMap<uuid::Uuid, String> = daily_summary_rows
        .iter()
        .fold(std::collections::HashMap::new(), |mut acc, row| {
            let agent_id: uuid::Uuid = row.get("agent_id");
            let game_day: i64 = row.get("game_day");
            let summary: String = row.get("summary");
            acc.entry(agent_id)
                .and_modify(|s| {
                    *s += &format!("\n[游戏日 {}] {}", game_day, summary);
                })
                .or_insert_with(|| format!("[游戏日 {}] {}", game_day, summary));
            acc
        });

    // 批量查询：死亡状态（使用窗口函数）
    let death_rows = sqlx::query(
        r#"
        WITH AgentDeaths AS (
            SELECT
                agent_id,
                tick_id,
                is_alive,
                LAG(is_alive) OVER (PARTITION BY agent_id ORDER BY tick_id) as prev_alive
            FROM agent_states
            WHERE tick_id BETWEEN $1 AND $2
        )
        SELECT DISTINCT agent_id
        FROM AgentDeaths
        WHERE is_alive = false AND prev_alive = true
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询死亡状态失败")?;

    let death_agents: std::collections::HashSet<uuid::Uuid> =
        death_rows.iter().map(|row| row.get("agent_id")).collect();

    // 组装结果
    let agents: Vec<AgentInfo> = rows
        .into_iter()
        .map(|row| {
            let agent_id: uuid::Uuid = row.get("agent_id");
            let name: String = row.get("name");
            let location: String = row.get("location");

            let (actions_count, top_actions) =
                agent_actions.remove(&agent_id).unwrap_or((0, Vec::new()));
            let actions_count = actions_count as i32;
            let top_actions: Vec<(String, i32)> = top_actions
                .into_iter()
                .map(|(k, v)| (k, v as i32))
                .collect();

            // 优先使用每日摘要（agent_daily_summaries），fallback 到 per-tick narratives
            let narratives: Vec<String> = if let Some(daily) = daily_summaries_map.get(&agent_id) {
                vec![daily.clone()]
            } else {
                narratives_map
                    .get(&agent_id)
                    .cloned()
                    .map(|n| vec![n])
                    .unwrap_or_default()
            };

            let died_this_period = death_agents.contains(&agent_id);

            AgentInfo {
                agent_id,
                name,
                location,
                actions_count,
                top_actions,
                narratives,
                died_this_period,
            }
        })
        .collect();

    Ok(agents)
}

/// 采集关键事件
async fn collect_highlights(
    db_pool: &crate::db::DbPool,
    period_start: i64,
    period_end: i64,
) -> Result<Vec<Highlight>> {
    // 批量查询：死亡事件（已包含 agent name）
    let death_rows = sqlx::query(
        r#"
        WITH StateChanges AS (
            SELECT
                s.agent_id,
                s.tick_id,
                s.is_alive,
                LAG(s.is_alive) OVER (PARTITION BY s.agent_id ORDER BY s.tick_id) as prev_alive
            FROM agent_states s
            WHERE s.tick_id BETWEEN $1 AND $2
        )
        SELECT
            sc.agent_id,
            sc.tick_id,
            a.name
        FROM StateChanges sc
        INNER JOIN agents a ON sc.agent_id = a.agent_id
        WHERE sc.is_alive = false AND sc.prev_alive = true
        ORDER BY sc.tick_id
        LIMIT 20
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询死亡事件失败")?;

    let mut highlights: Vec<Highlight> = death_rows
        .into_iter()
        .map(|row| {
            let agent_id: uuid::Uuid = row.get("agent_id");
            let tick_id: i64 = row.get("tick_id");
            let name: String = row.get("name");
            Highlight {
                tick_id,
                event_type: "death".to_string(),
                description: format!("{} 在江湖中陨落", name),
                agent_id: Some(agent_id),
                agent_name: Some(name),
            }
        })
        .collect();

    // 批量查询：其他事件（speak, attack, give）
    let event_rows = sqlx::query(
        r#"
        SELECT l.tick_id, l.agent_id, l.action_type, l.narrative, l.result_message, a.name
        FROM agent_action_logs l
        INNER JOIN agents a ON l.agent_id = a.agent_id
        WHERE l.tick_id BETWEEN $1 AND $2
        AND (
            (l.action_type = 'speak' AND l.narrative IS NOT NULL)
            OR (l.action_type = 'attack' AND l.result = 'success')
            OR (l.action_type = 'give' AND l.result = 'success')
        )
        ORDER BY l.tick_id
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询关键事件失败")?;

    // 按类型采样（保留有序性，随机性通过打乱后取前 N 实现）
    let mut dialogues = Vec::new();
    let mut combats = Vec::new();
    let mut socials = Vec::new();

    for row in event_rows {
        let tick_id: i64 = row.get("tick_id");
        let agent_id: uuid::Uuid = row.get("agent_id");
        let action_type: String = row.get("action_type");
        let agent_name: String = row.get("name");

        let highlight = match action_type.as_str() {
            "说话" => {
                let narrative: String = row.get("narrative");
                Highlight {
                    tick_id,
                    event_type: "dialogue".to_string(),
                    description: truncate_string(&narrative, 100),
                    agent_id: Some(agent_id),
                    agent_name: Some(agent_name),
                }
            }
            "攻击" => {
                let result_message: Option<String> = row.get("result_message");
                Highlight {
                    tick_id,
                    event_type: "combat".to_string(),
                    description: result_message
                        .map(|m| format!("{}: {}", agent_name, m))
                        .unwrap_or_else(|| format!("{} 发起了一场战斗", agent_name)),
                    agent_id: Some(agent_id),
                    agent_name: Some(agent_name),
                }
            }
            "给予" => {
                let result_message: Option<String> = row.get("result_message");
                Highlight {
                    tick_id,
                    event_type: "social".to_string(),
                    description: result_message
                        .map(|m| format!("{} 赠出: {}", agent_name, m))
                        .unwrap_or_else(|| format!("{} 赠出物品", agent_name)),
                    agent_id: Some(agent_id),
                    agent_name: Some(agent_name),
                }
            }
            _ => continue,
        };

        match action_type.as_str() {
            "说话" => dialogues.push(highlight),
            "攻击" => combats.push(highlight),
            "给予" => socials.push(highlight),
            _ => {}
        }
    }

    // 打乱后取前 N（用 tick_id 作为伪随机种子避免每次生成不同结果）
    let shuffle_and_take = |v: Vec<Highlight>, n: usize| {
        let mut shuffled = v;
        // 简单的 Fisher-Yates 变体，使用 period_start 作为种子
        let seed = period_start as usize;
        for i in 0..shuffled.len() {
            let j = (i + seed + i * 17) % shuffled.len();
            shuffled.swap(i, j);
        }
        shuffled.into_iter().take(n).collect::<Vec<_>>()
    };

    highlights.extend(shuffle_and_take(dialogues, 10));
    highlights.extend(shuffle_and_take(combats, 5));
    highlights.extend(shuffle_and_take(socials, 5));

    // 按 tick_id 排序
    highlights.sort_by_key(|h| h.tick_id);

    Ok(highlights)
}

/// 采集统计数据
async fn collect_stats(
    db_pool: &crate::db::DbPool,
    period_start: i64,
    period_end: i64,
) -> Result<(ActionStats, Vec<LocationStat>)> {
    // 动作类型统计
    let action_type_rows = sqlx::query(
        "SELECT action_type, COUNT(*) as cnt FROM agent_action_logs WHERE tick_id BETWEEN $1 AND $2 GROUP BY action_type"
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询动作类型统计失败")?;

    let mut by_type = HashMap::new();
    let mut total = 0i32;

    for row in action_type_rows {
        let action_type: String = row.get("action_type");
        let cnt: i64 = row.get("cnt");
        total += cnt as i32;
        by_type.insert(action_type, cnt as i32);
    }

    // 成功率
    let success_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_action_logs WHERE tick_id BETWEEN $1 AND $2 AND result = 'success'"
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_one(db_pool)
    .await
    .context("查询成功率失败")?;

    let success_rate = if total > 0 {
        success_count as f64 / total as f64
    } else {
        0.0
    };

    let action_stats = ActionStats {
        total,
        by_type,
        success_rate,
    };

    // 地点分布
    let location_rows = sqlx::query(
        r#"
        SELECT node_id, COUNT(*) as cnt
        FROM agent_states
        WHERE tick_id BETWEEN $1 AND $2 AND is_alive = true
        GROUP BY node_id
        ORDER BY cnt DESC
        LIMIT 10
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_all(db_pool)
    .await
    .context("查询地点分布失败")?;

    let total_location_count: i64 = location_rows.iter().map(|r| r.get::<i64, _>("cnt")).sum();

    let location_stats: Vec<LocationStat> = location_rows
        .iter()
        .map(|r| {
            let count: i64 = r.get("cnt");
            LocationStat {
                location: r.get("node_id"),
                count: count as i32,
                percentage: if total_location_count > 0 {
                    (count as f64 / total_location_count as f64) * 100.0
                } else {
                    0.0
                },
            }
        })
        .collect();

    Ok((action_stats, location_stats))
}

/// 采集死亡人数
async fn collect_deaths(
    db_pool: &crate::db::DbPool,
    period_start: i64,
    period_end: i64,
) -> Result<i32> {
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT agent_id)
        FROM agent_states s1
        WHERE s1.tick_id BETWEEN $1 AND $2
        AND s1.is_alive = false
        AND EXISTS (
            SELECT 1 FROM agent_states s2
            WHERE s2.agent_id = s1.agent_id
            AND s2.tick_id < s1.tick_id
            AND s2.is_alive = true
        )
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_one(db_pool)
    .await
    .context("查询死亡人数失败")?;

    Ok(count as i32)
}

/// 采集新生人数
///
/// 由于 agents 表只有 created_at（时间戳），无法直接映射到 tick_id。
/// 这里使用首次有动作记录的 agent 作为"新生"的代理指标。
/// 即：在本周期内首次出现在 action_logs 中的 agent 数量。
async fn collect_births(
    db_pool: &crate::db::DbPool,
    period_start: i64,
    period_end: i64,
) -> Result<i32> {
    // 查找在本周期内首次出现（在 period_start 之前没有任何动作记录）的 agent
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM (
            SELECT l.agent_id
            FROM agent_action_logs l
            WHERE l.tick_id BETWEEN $1 AND $2
            GROUP BY l.agent_id
            HAVING MIN(l.tick_id) >= $1 AND MIN(l.tick_id) <= $2
        ) AS new_in_period
        WHERE NOT EXISTS (
            SELECT 1 FROM agent_action_logs prev
            WHERE prev.agent_id = new_in_period.agent_id
            AND prev.tick_id < $1
        )
        "#,
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_one(db_pool)
    .await
    .context("查询新生人数失败")?;

    Ok(count as i32)
}

/// 获取 ticks_per_hour 配置
async fn get_ticks_per_hour() -> Option<i64> {
    crate::game_data::registry::TimeRegistry::get_config().map(|c| c.ticks_per_hour as i64)
}

/// 截断字符串（正确处理 UTF-8 字符边界）
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    // 确保截断点在字符边界上
    let end = s
        .char_indices()
        .nth(max_len.saturating_sub(3))
        .map(|(idx, _)| idx)
        .unwrap_or(s.len());
    format!("{}...", &s[..end])
}
