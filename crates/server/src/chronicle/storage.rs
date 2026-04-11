// ============================================================================
// 存储层
// ============================================================================
//
// 持久化到 chronicles 表
// ============================================================================

use anyhow::{Context, Result};
use sqlx::Row;

use super::collector::CollectedData;
use super::{AgentSummary, Chronicle, Highlight, LocationStat};

/// 存储群像传记
pub async fn store(
    db_pool: &crate::db::DbPool,
    data: &CollectedData,
    summary: &str,
) -> Result<Chronicle> {
    // 生成 chronicle_id
    let chronicle_id = generate_chronicle_id(db_pool).await?;

    // 转换数据结构
    let agent_summaries: Vec<AgentSummary> = data
        .agents
        .iter()
        .map(|a| AgentSummary {
            agent_id: a.agent_id,
            name: a.name.clone(),
            location: a.location.clone(),
            actions_count: a.actions_count,
            top_actions: a
                .top_actions
                .iter()
                .map(|(t, c)| format!("{}:{}", t, c))
                .collect(),
            narrative: a.narratives.first().cloned(),
            died_this_period: a.died_this_period,
        })
        .collect();

    let highlights_json =
        serde_json::to_value(&data.highlights).unwrap_or(serde_json::Value::Array(vec![]));

    let agent_summaries_json =
        serde_json::to_value(&agent_summaries).unwrap_or(serde_json::Value::Array(vec![]));

    let action_stats_json = serde_json::json!({
        "total": data.action_stats.total,
        "by_type": data.action_stats.by_type,
        "success_rate": data.action_stats.success_rate,
    });

    let location_stats_json =
        serde_json::to_value(&data.location_stats).unwrap_or(serde_json::Value::Array(vec![]));

    let raw_data = serde_json::json!({
        "period_start": data.period_start,
        "period_end": data.period_end,
        "game_day_start": data.game_day_start,
        "game_day_end": data.game_day_end,
        "season": data.season,
        "agents_count": data.agents.len(),
        "highlights_count": data.highlights.len(),
    });

    let row = sqlx::query_scalar::<_, (i64,)>(
        r#"
        INSERT INTO chronicles (
            chronicle_id, period_start, period_end,
            game_day_start, game_day_end, season,
            summary, agent_count, actions_count,
            highlights, agent_summaries, action_stats,
            location_stats, deaths, births, raw_data, status
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, 'template')
        RETURNING id
        "#,
    )
    .bind(&chronicle_id)
    .bind(data.period_start)
    .bind(data.period_end)
    .bind(data.game_day_start)
    .bind(data.game_day_end)
    .bind(&data.season)
    .bind(summary)
    .bind(data.agents.len() as i32)
    .bind(data.action_stats.total)
    .bind(highlights_json)
    .bind(agent_summaries_json)
    .bind(action_stats_json)
    .bind(location_stats_json)
    .bind(data.deaths)
    .bind(data.births)
    .bind(raw_data)
    .fetch_one(db_pool)
    .await
    .context("插入 chronicles 记录失败")?;

    let id = row.0;

    Ok(Chronicle {
        id,
        chronicle_id,
        period_start: data.period_start,
        period_end: data.period_end,
        game_day_start: data.game_day_start,
        game_day_end: data.game_day_end,
        season: data.season.clone(),
        summary: summary.to_string(),
        summary_llm: None,
        agent_count: data.agents.len() as i32,
        actions_count: data.action_stats.total,
        highlights: data.highlights.clone(),
        agent_summaries,
        action_stats: data.action_stats.clone(),
        location_stats: data.location_stats.clone(),
        deaths: data.deaths,
        births: data.births,
        status: "template".to_string(),
        created_at: chrono::Utc::now(),
    })
}

/// 更新 LLM 摘要
pub async fn update_llm_summary(
    db_pool: &crate::db::DbPool,
    chronicle_id: &str,
    summary_llm: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE chronicles
        SET summary_llm = $1,
            status = 'both'
        WHERE chronicle_id = $2
        "#,
    )
    .bind(summary_llm)
    .bind(chronicle_id)
    .execute(db_pool)
    .await
    .context("更新 LLM 摘要失败")?;

    Ok(())
}

/// 获取所有群像传记（列表）
pub async fn list_chronicles(
    db_pool: &crate::db::DbPool,
    limit: i32,
    offset: i32,
) -> Result<Vec<ChronicleMeta>> {
    let rows = sqlx::query(
        r#"
        SELECT id, chronicle_id, period_start, period_end,
               game_day_start, game_day_end, season,
               summary, agent_count, actions_count,
               deaths, births, status, created_at
        FROM chronicles
        ORDER BY period_start DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(db_pool)
    .await
    .context("查询 chronicles 列表失败")?;

    let chronicles: Vec<ChronicleMeta> = rows
        .iter()
        .map(|r| ChronicleMeta {
            id: r.get("id"),
            chronicle_id: r.get("chronicle_id"),
            period_start: r.get("period_start"),
            period_end: r.get("period_end"),
            game_day_start: r.get("game_day_start"),
            game_day_end: r.get("game_day_end"),
            season: r.get("season"),
            summary_preview: truncate_text(&r.get::<String, _>("summary"), 200),
            agent_count: r.get("agent_count"),
            actions_count: r.get("actions_count"),
            deaths: r.get("deaths"),
            births: r.get("births"),
            status: r.get("status"),
            created_at: r.get("created_at"),
        })
        .collect();

    Ok(chronicles)
}

/// 获取单个群像传记
pub async fn get_chronicle(
    db_pool: &crate::db::DbPool,
    chronicle_id: &str,
) -> Result<Option<Chronicle>> {
    let row = sqlx::query(
        r#"
        SELECT id, chronicle_id, period_start, period_end,
               game_day_start, game_day_end, season,
               summary, summary_llm, agent_count, actions_count,
               highlights, agent_summaries, action_stats,
               location_stats, deaths, births, status, created_at
        FROM chronicles
        WHERE chronicle_id = $1
        "#,
    )
    .bind(chronicle_id)
    .fetch_optional(db_pool)
    .await
    .context("查询 chronicle 详情失败")?;

    match row {
        Some(r) => {
            let highlights: Vec<Highlight> = r
                .get::<serde_json::Value, _>("highlights")
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();

            let agent_summaries: Vec<AgentSummary> = r
                .get::<serde_json::Value, _>("agent_summaries")
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();

            let location_stats: Vec<LocationStat> = r
                .get::<serde_json::Value, _>("location_stats")
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();

            let action_stats_json = r.get::<serde_json::Value, _>("action_stats");
            let action_stats = super::ActionStats {
                total: action_stats_json
                    .get("total")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32,
                by_type: action_stats_json
                    .get("by_type")
                    .and_then(|v| v.as_object())
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_i64().map(|n| (k.clone(), n as i32)))
                            .collect()
                    })
                    .unwrap_or_default(),
                success_rate: action_stats_json
                    .get("success_rate")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
            };

            Ok(Some(Chronicle {
                id: r.get("id"),
                chronicle_id: r.get("chronicle_id"),
                period_start: r.get("period_start"),
                period_end: r.get("period_end"),
                game_day_start: r.get("game_day_start"),
                game_day_end: r.get("game_day_end"),
                season: r.get("season"),
                summary: r.get("summary"),
                summary_llm: r.get("summary_llm"),
                agent_count: r.get("agent_count"),
                actions_count: r.get("actions_count"),
                highlights,
                agent_summaries,
                action_stats,
                location_stats,
                deaths: r.get("deaths"),
                births: r.get("births"),
                status: r.get("status"),
                created_at: r.get("created_at"),
            }))
        }
        None => Ok(None),
    }
}

/// 获取 chronicle 总数
pub async fn count_chronicles(db_pool: &crate::db::DbPool) -> Result<i64> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chronicles")
        .fetch_one(db_pool)
        .await
        .context("查询 chronicle 总数失败")?;
    Ok(count)
}

/// 生成 chronicle_id (C-001, C-002, ...)
async fn generate_chronicle_id(db_pool: &crate::db::DbPool) -> Result<String> {
    // CAST(SUBSTRING(...) AS INT) 返回 INT4 (i32)
    let max_id: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(CAST(SUBSTRING(chronicle_id FROM 3) AS INT)), 0) FROM chronicles WHERE chronicle_id LIKE 'C-%'"
    )
    .fetch_one(db_pool)
    .await
    .map_err(|e| anyhow::anyhow!("生成 chronicle_id 失败: {}", e))?;

    Ok(format!("C-{:03}", max_id + 1))
}

/// 截断文本
fn truncate_text(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Chronicle 列表元数据
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChronicleMeta {
    pub id: i64,
    pub chronicle_id: String,
    pub period_start: i64,
    pub period_end: i64,
    pub game_day_start: i32,
    pub game_day_end: i32,
    pub season: String,
    pub summary_preview: String,
    pub agent_count: i32,
    pub actions_count: i32,
    pub deaths: i32,
    pub births: i32,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
