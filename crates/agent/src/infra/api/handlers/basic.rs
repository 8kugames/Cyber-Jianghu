// 通用工具方法
// ============================================================================

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Serialize;
use uuid::Uuid;

use cyber_jianghu_protocol::{ActionType, Intent};

use super::context::{
    ContextResponse, create_attributes_glimpse, generate_context_markdown,
    generate_context_markdown_no_relationship,
};
use super::dto::HealthResponse;
use super::{HttpApiState, IntentRequest};

#[derive(Serialize)]
pub(crate) struct ErrorResponse {
    pub(crate) error_code: String,
    pub(crate) message: String,
}

/// 解析 tick_id：优先使用请求中的值，否则使用当前状态的 tick_id
/// 如果当前没有状态，则拒绝请求
pub(crate) async fn resolve_tick_id_or_reject(
    req_tick_id: Option<i64>,
    state: &HttpApiState,
) -> Result<i64, axum::response::Response> {
    if let Some(tick_id) = req_tick_id {
        return Ok(tick_id);
    }

    let current = state.current_state.read().await;
    match current.as_ref() {
        Some(world_state) => Ok(world_state.tick_id),
        None => {
            let resp = (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error_code: "tick_state_unavailable".to_string(),
                    message: "World state is not available yet".to_string(),
                }),
            )
                .into_response();
            Err(resp)
        }
    }
}

// ============================================================================

// 基础端点 Handlers
// ============================================================================

/// Health check handler
pub(crate) async fn health_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    let agent_id = *state.agent_id.read().await;

    let is_dead = state.is_dead.load(std::sync::atomic::Ordering::Relaxed);
    let response = HealthResponse {
        status: if is_dead { "dead" } else { "ok" }.to_string(),
        agent_id: if agent_id.is_nil() {
            None
        } else {
            Some(agent_id.to_string())
        },
        tick_id: current.as_ref().map(|s| s.tick_id),
    };
    Json(response)
}

/// Get current state handler
pub(crate) async fn get_state_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    match current.as_ref() {
        Some(world_state) => Json(world_state.clone()).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Get formatted context handler (使用叙事化描述，不暴露数值)
pub(crate) async fn get_context_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    let agent_id = *state.agent_id.read().await;

    // 读取托梦内容（不消费 — lifecycle.rs 已在决策周期中消费）
    let dream_thought = state.peek_dream().await;

    match current.as_ref() {
        Some(world_state) => {
            let context = {
                let store_arc = state
                    .relationship_store
                    .read()
                    .expect("rwlock poisoned")
                    .clone();
                if let Some(store) = store_arc.as_ref() {
                    generate_context_markdown(world_state, store, dream_thought.as_deref())
                } else {
                    generate_context_markdown_no_relationship(world_state, dream_thought.as_deref())
                }
            };

            // 读取决策上下文快照（enrichment）
            let enrichment = state
                .decision_context_snapshot
                .read()
                .await
                .as_ref()
                .map(|s| super::context::ContextEnrichment {
                    memory_context: s.memory_context.clone(),
                    summary_context: s.summary_context.clone(),
                    outcome_section: s.outcome_section.clone(),
                    action_descriptions: s.action_descriptions.clone(),
                    action_field_hints: s.action_field_hints.clone(),
                    last_execution_result: s.last_execution_result.clone(),
                });

            Json(ContextResponse {
                context,
                tick_id: world_state.tick_id,
                agent_id: agent_id.to_string(),
                enrichment,
            })
            .into_response()
        }
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Get attributes handler - "梦中一瞥" API
///
/// 返回当前属性数值，但警告此数据是一次性的，禁止存储到记忆系统
///
/// 格式说明：
/// - 显示格式：{display_name}: {value_str}
/// - 先天属性（growable）：{当前} ({上限})
/// - 状态值：{当前}/{最大}
/// - 派生属性：{计算值}
pub(crate) async fn get_attributes_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    match current.as_ref() {
        Some(world_state) => {
            let glimpse = create_attributes_glimpse(world_state);
            Json(glimpse).into_response()
        }
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Submit intent handler (完全数据驱动)
#[allow(dead_code)]
pub(crate) async fn submit_intent_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<IntentRequest>,
) -> impl IntoResponse {
    let tick_id = match resolve_tick_id_or_reject(req.tick_id, &state).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let current_tick = state
        .current_state
        .read()
        .await
        .as_ref()
        .map(|s| s.tick_id)
        .unwrap_or(0);
    if tick_id < current_tick {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "intent_expired",
                "message": format!("Intent tick {} is older than current tick {}", tick_id, current_tick),
                "current_tick": current_tick,
                "retry_suggestion": "Please fetch the latest state and submit intent for the new tick."
            })),
        )
            .into_response();
    }

    // 从共享状态读取最新的 agent_id（注册后会被更新）
    let state_agent_id = *state.agent_id.read().await;
    let agent_id = req
        .agent_id
        .as_ref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or(state_agent_id);

    let action_type: ActionType = req.action_type.into();
    let action_type_str = action_type.to_string();

    // "narrative" 是三魂架构的内部 sentinel，不应通过 HTTP API 提交
    if action_type_str == "narrative" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "action_type 'narrative' is an internal sentinel, use a valid action type"
            })),
        )
            .into_response();
    }
    let intent = if let Some(id_str) = &req.intent_id {
        if let Ok(id) = Uuid::parse_str(id_str) {
            Intent::new_with_id(id, agent_id, tick_id, action_type, req.action_data)
        } else {
            Intent::new(agent_id, tick_id, action_type, req.action_data)
        }
    } else {
        Intent::new(agent_id, tick_id, action_type, req.action_data)
    };

    // 添加 thought_log（如果有）
    let intent = if let Some(ref thought) = req.thought_log {
        intent.with_thought(thought.clone())
    } else {
        intent
    };

    // 记录到 IntentHistoryStore（用于经历日志查询）
    if let Some(history) = state.intent_history.read().await.as_ref() {
        history
            .record_intent(
                tick_id,
                0,
                intent.intent_id,
                action_type_str,
                req.thought_log.clone(),
            )
            .await;
    }

    let intent_id = intent.intent_id;
    let submitted_tick = tick_id;
    let submitted_action = intent.action_type.to_string();

    match state.intent_tx.send(intent).await {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "submitted",
                "intent_id": intent_id,
                "tick_id": submitted_tick,
                "action_type": submitted_action
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "channel_closed",
                "message": format!("Failed to submit intent: {}", e)
            })),
        )
            .into_response(),
    }
}

// ============================================================================
