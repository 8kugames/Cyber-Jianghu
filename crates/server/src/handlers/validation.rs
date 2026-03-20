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
    let all_states = match crate::db::get_all_alive_agents_latest_states(&state.db_pool).await {
        Ok(agents) => agents,
        Err(e) => {
            tracing::error!("Failed to get all agent states: {}", e);
            Vec::new()
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

    // Simple validation - check if agent is alive
    if !agent_state.is_alive {
        return Ok(Json(ActionValidationResponse {
            valid: false,
            reason: Some("Agent is dead".to_string()),
            suggestion: Some("Agent 已死亡，无法执行任何动作".to_string()),
        }));
    }

    // Build intent for validation (not actually used in this simplified version)
    let _intent = Intent {
        intent_id: Uuid::new_v4(),
        agent_id,
        tick_id: current_tick_id,
        thought_log: None,
        action_type,
        action_data: req.data,
        priority: 5,
    };

    // For now, just return valid for alive agents
    // Full validation logic is in actions/validator.rs which is private
    debug!("Action validation passed for agent: {}", agent_id);
    Ok(Json(ActionValidationResponse {
        valid: true,
        reason: None,
        suggestion: None,
    }))
}

/// Generate suggestion based on error（预留：智能动作建议）
#[allow(dead_code)]
fn generate_suggestion(action_type: &ActionType, error: &str) -> Option<String> {
    if error.contains("AgentDead") {
        Some("Agent 已死亡，无法执行任何动作".to_string())
    } else if error.contains("TargetNotFound") {
        Some("目标 Agent 不在当前位置".to_string())
    } else if error.contains("TargetDead") {
        Some("目标 Agent 已死亡".to_string())
    } else if error.contains("属性") || error.contains("不足") {
        match action_type.as_str() {
            "attack" => Some("HP 太低，无法攻击，建议先休息".to_string()),
            "move" => Some("体力不足，无法移动，建议先休息".to_string()),
            _ => Some("属性不足，无法执行此动作".to_string()),
        }
    } else if error.contains("对话内容") {
        Some("请提供非空的对话内容".to_string())
    } else if error.contains("物品") {
        Some("请检查物品 ID 是否正确".to_string())
    } else {
        Some("请检查动作参数是否正确".to_string())
    }
}
