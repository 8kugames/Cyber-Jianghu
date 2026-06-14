use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

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
    let total_groups: i64 = sqlx::query("SELECT COUNT(*) FROM action_evolution_proposal_groups")
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
            group_status: r
                .get::<Option<String>, _>(4)
                .unwrap_or_else(|| "ungrouped".to_string()),
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

#[derive(Serialize)]
pub struct ProposalGroupSummary {
    pub id: uuid::Uuid,
    pub similarity_key: String,
    pub primary_soul: Option<String>,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn get_proposal_groups(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let pool = &state.db_pool;
    let status_filter = params.get("status").map(|s| s.as_str()).unwrap_or("all");

    let rows = if status_filter == "all" {
        sqlx::query(
            "SELECT id, similarity_key, primary_soul, status, created_at, updated_at
             FROM action_evolution_proposal_groups ORDER BY updated_at DESC LIMIT 50",
        )
        .fetch_all(pool)
        .await
    } else {
        sqlx::query(
            "SELECT id, similarity_key, primary_soul, status, created_at, updated_at
             FROM action_evolution_proposal_groups WHERE status = $1 ORDER BY updated_at DESC LIMIT 50",
        )
        .bind(status_filter)
        .fetch_all(pool)
        .await
    }
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let groups: Vec<ProposalGroupSummary> = rows
        .iter()
        .map(|r| ProposalGroupSummary {
            id: r.get(0),
            similarity_key: r.get(1),
            primary_soul: r.get(2),
            status: r.get(3),
            created_at: r.get(4),
            updated_at: r.get(5),
        })
        .collect();

    Ok(Json(serde_json::json!({ "groups": groups })))
}

pub async fn get_proposal_group_detail(
    Path(group_id): Path<uuid::Uuid>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let pool = &state.db_pool;
    let row = sqlx::query(
        "SELECT id, similarity_key, primary_soul, co_reviewers, governance_topics,
                status, votes, final_decision, dissent_log, proposal_ids, created_at, updated_at
         FROM action_evolution_proposal_groups WHERE id = $1",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    match row {
        Some(r) => Ok(Json(serde_json::json!({
            "id": r.try_get::<uuid::Uuid, _>(0).ok(),
            "similarity_key": r.try_get::<String, _>(1).ok(),
            "primary_soul": r.try_get::<Option<String>, _>(2).ok(),
            "co_reviewers": r.try_get::<serde_json::Value, _>(3).ok(),
            "governance_topics": r.try_get::<serde_json::Value, _>(4).ok(),
            "status": r.try_get::<String, _>(5).ok(),
            "votes": r.try_get::<serde_json::Value, _>(6).ok(),
            "final_decision": r.try_get::<Option<String>, _>(7).ok(),
            "dissent_log": r.try_get::<serde_json::Value, _>(8).ok(),
            "proposal_ids": r.try_get::<serde_json::Value, _>(9).ok(),
            "created_at": r.try_get::<chrono::DateTime<chrono::Utc>, _>(10).ok(),
            "updated_at": r.try_get::<chrono::DateTime<chrono::Utc>, _>(11).ok(),
        }))),
        None => Err(axum::http::StatusCode::NOT_FOUND),
    }
}

#[derive(Deserialize)]
pub struct AdminActionRequest {
    pub action: String,
    pub reason: String,
}

pub async fn admin_action_on_group(
    Path(group_id): Path<uuid::Uuid>,
    State(state): State<Arc<AppState>>,
    Json(req): Json<AdminActionRequest>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let pool = &state.db_pool;
    let new_status = match req.action.as_str() {
        "approve" => "approved",
        "reject" => "rejected",
        _ => return Err(axum::http::StatusCode::BAD_REQUEST),
    };

    sqlx::query(
        "UPDATE action_evolution_proposal_groups
         SET status = $1, final_decision = $2, updated_at = NOW()
         WHERE id = $3",
    )
    .bind(new_status)
    .bind(&req.reason)
    .bind(group_id)
    .execute(pool)
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    // 管理员 approve 触发完整 auto-evolve 流水线
    if new_status == "approved" {
        let Some(ref gov) = state.governance else {
            return Ok(Json(
                serde_json::json!({"status": "ok", "new_status": new_status}),
            ));
        };

        // 读取 group 的 proposal_ids
        let group_row =
            sqlx::query("SELECT proposal_ids FROM action_evolution_proposal_groups WHERE id = $1")
                .bind(group_id)
                .fetch_optional(pool)
                .await
                .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

        if let Some(row) = group_row {
            let proposal_ids: Vec<uuid::Uuid> =
                serde_json::from_value(row.get::<serde_json::Value, _>(0)).unwrap_or_default();

            let config_dir = crate::paths::get_config_dir();

            // 逐个 proposal 生成 action config 并写入 actions.yaml
            for pid in &proposal_ids {
                match gov.proposal_store.get_proposal(*pid).await {
                    Ok(Some(evidence)) => {
                        match crate::governance::auto_evolve::generate_action_config(&evidence) {
                            Ok((action_name, entry)) => {
                                if let Err(e) =
                                    crate::governance::action_writer::append_action_to_yaml(
                                        &config_dir,
                                        &action_name,
                                        &entry,
                                    )
                                {
                                    warn!(
                                        action_name = %action_name,
                                        error = %e,
                                        "管理员 approve: 写入 actions.yaml 失败"
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    proposal_id = %pid,
                                    error = %e,
                                    "管理员 approve: 生成 action config 失败"
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        warn!(proposal_id = %pid, "管理员 approve: proposal 不存在");
                    }
                    Err(e) => {
                        warn!(
                            proposal_id = %pid,
                            error = %e,
                            "管理员 approve: 获取 proposal 失败"
                        );
                    }
                }
            }

            // 重载 ActionRegistry
            match crate::game_data::loaders::load_actions(&config_dir) {
                Ok(new_actions) => {
                    state.game_data.update_actions(new_actions);
                    info!("管理员 approve: ActionRegistry 已更新");
                }
                Err(e) => {
                    warn!("管理员 approve: ActionRegistry 重载失败: {}", e);
                }
            }

            // 刷新 CapabilityManifest
            gov.engine.write().await.reload_manifest().await;

            // 广播 ConfigUpdate
            let actions_content =
                std::fs::read_to_string(config_dir.join("actions.yaml")).unwrap_or_default();
            let config_update = cyber_jianghu_protocol::messages::ServerMessage::ConfigUpdate {
                config_type: "actions".to_string(),
                update_type: "full".to_string(),
                version: chrono::Utc::now().to_rfc3339(),
                content: serde_json::json!({"yaml": actions_content}),
                content_hash: None,
                updated_items: vec![],
                removed_items: vec![],
            };
            if let Err(e) =
                crate::websocket::broadcast_config_update(config_update, &gov.connection_manager)
                    .await
            {
                warn!("管理员 approve: broadcast 失败: {}", e);
            }
        }
    }

    Ok(Json(
        serde_json::json!({"status": "ok", "new_status": new_status}),
    ))
}
