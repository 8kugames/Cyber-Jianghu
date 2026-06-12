use axum::{Json, extract::State};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

use crate::state::AppState;

#[derive(Serialize)]
pub struct SoulGroupStats {
    pub soul: String,
    pub count: i64,
    pub status: String,
}

#[derive(Serialize)]
pub struct ActionEvolutionStats {
    pub total_proposals: i64,
    pub total_groups: i64,
    pub by_status: Vec<StatusGroupStats>,
    pub by_soul: Vec<SoulGroupStats>,
    pub recent_proposals: Vec<RecentProposal>,
}

#[derive(Serialize)]
pub struct StatusGroupStats {
    pub status: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct RecentProposal {
    pub id: uuid::Uuid,
    pub proposed_action_type: String,
    pub rationale: String,
    pub primary_soul: Option<String>,
    pub group_status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn get_action_evolution_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ActionEvolutionStats>, axum::http::StatusCode> {
    let pool = &state.db_pool;

    // Total proposals
    let total_proposals: i64 = sqlx::query("SELECT COUNT(*) FROM action_evolution_proposals")
        .fetch_one(pool)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
        .get(0);

    // Total groups
    let total_groups: i64 =
        sqlx::query("SELECT COUNT(*) FROM action_evolution_proposal_groups")
            .fetch_one(pool)
            .await
            .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
            .get(0);

    // Groups by status
    let status_rows = sqlx::query(
        "SELECT status, COUNT(*) as count FROM action_evolution_proposal_groups GROUP BY status",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let by_status: Vec<StatusGroupStats> = status_rows
        .iter()
        .map(|r| StatusGroupStats {
            status: r.get(0),
            count: r.get(1),
        })
        .collect();

    // Groups by primary_soul and status
    let soul_rows = sqlx::query(
        "SELECT primary_soul, COUNT(*) as count, status
         FROM action_evolution_proposal_groups
         GROUP BY primary_soul, status",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let by_soul: Vec<SoulGroupStats> = soul_rows
        .iter()
        .map(|r| SoulGroupStats {
            soul: r.get::<Option<String>, _>(0).unwrap_or_default(),
            count: r.get(1),
            status: r.get(2),
        })
        .collect();

    // Recent proposals (last 20)
    let recent_rows = sqlx::query(
        "SELECT p.id, p.proposed_action_type, p.rationale, g.primary_soul, g.status, p.created_at
         FROM action_evolution_proposals p
         LEFT JOIN action_evolution_proposal_groups g
           ON g.id = (SELECT id FROM action_evolution_proposal_groups
                      WHERE proposal_ids ? p.id::text LIMIT 1)
         ORDER BY p.created_at DESC
         LIMIT 20",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let recent_proposals: Vec<RecentProposal> = recent_rows
        .iter()
        .map(|r| RecentProposal {
            id: r.get(0),
            proposed_action_type: r.get(1),
            rationale: r.get(2),
            primary_soul: r.get(3),
            group_status: r.get::<Option<String>, _>(4).unwrap_or_else(|| "ungrouped".to_string()),
            created_at: r.get(5),
        })
        .collect();

    Ok(Json(ActionEvolutionStats {
        total_proposals,
        total_groups,
        by_status,
        by_soul,
        recent_proposals,
    }))
}
