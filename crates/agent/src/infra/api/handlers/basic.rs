// 通用工具方法
// ============================================================================

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Serialize;

use super::HttpApiState;
use super::context::{
    ContextResponse, create_attributes_glimpse, generate_context_markdown,
    generate_context_markdown_no_relationship,
};
use super::dto::HealthResponse;

#[derive(Serialize)]
pub(crate) struct ErrorResponse {
    pub(crate) error_code: String,
    pub(crate) message: String,
}

/// 解析 tick_id：优先使用请求中的值，否则使用当前状态的 tick_id
/// 如果当前没有状态，则拒绝请求（Err 装箱控制 Result 体积，clippy result_large_err）
pub(crate) async fn resolve_tick_id_or_reject(
    req_tick_id: Option<i64>,
    state: &HttpApiState,
) -> Result<i64, Box<axum::response::Response>> {
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
            Err(Box::new(resp))
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

/// 协议握手 handler
///
/// GET /api/v1/version（公开端点，无需认证）
///
/// 返回 agent/protocol/server 三段版本。server_version 实时向 server 的
/// /api/v1/version 拉取（2s 超时）；server 不可达时为 null，不阻断握手。
pub(crate) async fn version_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let server_version = fetch_server_version(&state).await;
    Json(serde_json::json!({
        "agent_version": env!("CARGO_PKG_VERSION"),
        "protocol_version": cyber_jianghu_protocol::PROTOCOL_VERSION,
        "server_version": server_version,
    }))
}

/// 从 server 拉取 server_version（失败返回 None，不作为错误处理）
async fn fetch_server_version(state: &HttpApiState) -> Option<String> {
    let server_http_url = state.server_http_url.read().await.clone();
    let url = format!("{}/api/v1/version", server_http_url);
    let resp = reqwest::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("server_version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
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
/// - 基础属性值直接以整数展示，派生属性保留三位小数
/// - 类别由 attribute_categories 配置定义，不硬编码属性名/类
pub(crate) async fn get_attributes_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    let narrative = state.narrative_config.read().await.clone();
    match current.as_ref() {
        Some(world_state) => {
            let glimpse = create_attributes_glimpse(world_state, narrative.as_ref());
            Json(glimpse).into_response()
        }
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

// ============================================================================
