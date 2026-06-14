use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use cyber_jianghu_protocol::types::governance::GovernanceTopic;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct ProposalRequest {
    pub agent_id: Uuid,
    pub tick_id: i64,
    pub proposed_action_type: String,
    /// Agent intent 完整参数（target / item / quantity 等），供伏羲 LLM 审议
    #[serde(default)]
    pub action_data: serde_json::Value,
    /// Agent 端可不传，server 端由 TopicClassifier 根据 IR 自动分类
    #[serde(default)]
    pub governance_topics: Vec<GovernanceTopic>,
    #[serde(default)]
    pub topic_confidence: HashMap<GovernanceTopic, f64>,
    pub rationale: String,
}

pub async fn submit_proposal(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ProposalRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let gov = state
        .governance
        .as_ref()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    // Phase 0：伏羲单 soul，effect_refs 由 LLM 审议时推断，提议阶段为空
    let effect_refs: Vec<String> = vec![];
    let classification =
        gov.classifier
            .classify(&effect_refs, &req.governance_topics, &req.topic_confidence);

    let primary_soul = gov.engine.route_for_topics(&classification.topics);

    let evidence = super::ProposalEvidence {
        agent_id: req.agent_id,
        tick_id: req.tick_id,
        proposed_action_type: req.proposed_action_type,
        action_data: req.action_data,
        governance_topics: classification.topics.clone(),
        topic_confidence: classification.confidence.clone(),
        rationale: req.rationale,
    };

    let proposal_id = gov
        .proposal_store
        .insert_proposal(&evidence)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let similarity_key = format!("action:{}", evidence.proposed_action_type);
    let group_id = gov
        .proposal_store
        .upsert_proposal_group(
            &similarity_key,
            proposal_id,
            &classification.topics,
            primary_soul.as_deref(),
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "status": "accepted",
        "proposal_id": proposal_id,
        "group_id": group_id,
        "primary_soul": primary_soul,
    })))
}
