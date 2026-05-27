// 验证 API Handlers
// ============================================================================

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tracing::error;
use uuid::Uuid;

use crate::core::utils::build_world_context;
use crate::soul::reflector::{
    PersonaInfo, PipelineValidationResult, ValidationRequest, ValidationRuntimeConfig,
};
use cyber_jianghu_protocol::Intent;

use super::HttpApiState;
use super::basic::resolve_tick_id_or_reject;
use super::dto::{ValidateRequest, ValidateResponse};

/// 验证 Intent（数据驱动）
pub(crate) async fn validate_intent_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<ValidateRequest>,
) -> impl IntoResponse {
    if req.action_type.trim().is_empty() {
        return Json(ValidateResponse {
            valid: false,
            reason: Some("action_type cannot be empty".to_string()),
            rejection_type: None,
            narrative: None,
        })
        .into_response();
    }

    // "narrative" 是三魂架构的内部 sentinel，不应通过 HTTP API 提交
    if req.action_type.trim() == "narrative" {
        return Json(ValidateResponse {
            valid: false,
            reason: Some(
                "action_type 'narrative' is an internal sentinel, not a valid action".to_string(),
            ),
            rejection_type: None,
            narrative: None,
        })
        .into_response();
    }

    let tick_id = match resolve_tick_id_or_reject(req.tick_id, &state).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let validator = match &state.intent_validator {
        Some(v) => v,
        None => {
            return Json(ValidateResponse {
                valid: true,
                reason: None,
                rejection_type: None,
                narrative: None,
            })
            .into_response();
        }
    };

    let state_agent_id = *state.agent_id.read().await;
    let agent_id = req
        .agent_id
        .as_ref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or(state_agent_id);

    let intent = Intent::new(agent_id, tick_id, req.action_type, req.action_data);

    let persona_info = PersonaInfo {
        name: req.persona_name.clone(),
        gender: req.persona_gender.unwrap_or_else(|| "未知".to_string()),
        age: req.persona_age.unwrap_or(28),
        personality: req.persona_personality.unwrap_or_default(),
        values: req.persona_values.unwrap_or_default(),
    };

    let world_state = state.current_state.read().await.clone();
    let world_context = world_state
        .as_ref()
        .map(build_world_context)
        .unwrap_or_else(|| "No world state available".to_string());
    let graded_config = state
        .game_rules
        .read()
        .await
        .as_ref()
        .and_then(|rules| rules.intent_batch.as_ref())
        .map(|batch| batch.llm_validation.clone());

    // [TRAP_DEBT: TICKET-101] HTTP API 是无状态游离端点，无法获取内存中的连续跟随计数
    // 当前传入 0，意味着通过 HTTP 提交的单次意图将绕过防刷屏限制。
    // 预计修复方案：在 HttpApiState 中补充对 Agent 状态的只读引用，或由调用方传入。
    // 预计偿还时间：2026-06-01
    let max_consecutive_follow = crate::config::Config::from_file(&state.config_path)
        .map(|c| c.llm.max_consecutive_follow)
        .unwrap_or(crate::config::DEFAULT_MAX_CONSECUTIVE_FOLLOW);

    let validation_req = ValidationRequest {
        intent,
        persona: persona_info,
        world_context,
        world_state,
        runtime: ValidationRuntimeConfig {
            graded_config,
            consecutive_follow_count: 0,
            max_consecutive_follow,
            recent_same_type_decisions: vec![],
        },
    };

    match validator.validate(validation_req).await {
        Ok(PipelineValidationResult::Approved { narrative, .. }) => Json(ValidateResponse {
            valid: true,
            reason: None,
            rejection_type: None,
            narrative,
        })
        .into_response(),
        Ok(PipelineValidationResult::Rejected { reason, .. }) => Json(ValidateResponse {
            valid: false,
            reason: Some(reason),
            rejection_type: None,
            narrative: None,
        })
        .into_response(),
        Err(e) => {
            error!("[http] Validation error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Validation error: {}", e),
            )
                .into_response()
        }
    }
}

// ============================================================================
