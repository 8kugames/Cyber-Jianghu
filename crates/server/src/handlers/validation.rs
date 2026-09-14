// ============================================================================
// Action Validation Handler
// ============================================================================
//
// Provides HTTP API for action validation before execution
//
// POST /api/v1/validate-action
// ============================================================================

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::debug;

use crate::models::ActionType;
use crate::models::Intent;
use crate::state::AppState;
use uuid::Uuid;

/// Validate action request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionValidationRequest {
    /// Agent ID
    pub agent_id: String,
    /// Action type
    pub action: String,
    /// Action data (JSON)
    pub data: Option<serde_json::Value>,
}

/// Validate action response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionValidationResponse {
    /// Whether the action is valid
    pub valid: bool,
    /// Reason for invalidity (if invalid)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Suggestion for fixing the issue (if invalid)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// Validate action
///
/// Validates an action before execution, returning errors and suggestions
pub async fn validate_action(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ActionValidationRequest>,
) -> Result<Json<ActionValidationResponse>, StatusCode> {
    let agent_id = Uuid::parse_str(&req.agent_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    debug!(
        "Validating action for agent: {}, action: {}",
        agent_id, req.action
    );

    // Get current tick ID
    let current_tick_id = match crate::db::get_current_world_tick_id(&state.db_pool).await {
        Ok(tick_id) => tick_id,
        Err(e) => {
            tracing::error!("Failed to get current tick ID: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Get all alive agents to find the current agent
    // （查询失败直接 500：吞掉后会对合法 Agent 误报「Agent not found」）
    let all_states = match crate::db::get_all_alive_agents_latest_states(&state.db_pool).await {
        Ok(agents) => agents,
        Err(e) => {
            tracing::error!("Failed to get all agent states: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Find the current agent's state
    let agent_state = match all_states.iter().find(|a| a.agent_id == agent_id) {
        Some(state) => state,
        None => {
            return Ok(Json(ActionValidationResponse {
                valid: false,
                reason: Some("Agent not found".to_string()),
                suggestion: Some("请确保 Agent 已注册".to_string()),
            }));
        }
    };

    // Parse action type (数据驱动：接受任意字符串)
    let action_type = ActionType::new(&req.action);

    let intent = Intent {
        intent_id: Uuid::new_v4(),
        agent_id,
        tick_id: current_tick_id,
        thought_log: None,
        action_type,
        action_data: req.data,
        priority: 5,
        reflector_thought: None,
        chaos_marker: None,
        dream_marker: None,
        already_broadcast: false,
        session_id: None,
        subsequent_intents: vec![],
    };

    // 干跑接入真实校验器（与 IntentWorker 同一条 validate_action 链：
    // 类型解析/规则校验/持有预检全覆盖；死亡检查由其内部完成并给出可自纠文案）
    match crate::actions::validate_action(&intent, agent_state, &all_states, &state.db_pool).await {
        Ok(_) => {
            debug!("Action validation passed for agent: {}", agent_id);
            Ok(Json(ActionValidationResponse {
                valid: true,
                reason: None,
                suggestion: None,
            }))
        }
        Err(e) => Ok(Json(ActionValidationResponse {
            valid: false,
            reason: Some(e.to_string()),
            suggestion: None,
        })),
    }
}
