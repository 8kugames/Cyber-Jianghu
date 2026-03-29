// ============================================================================
// HTTP API Handlers - 所有 API 端点的处理器
// ============================================================================
//
// 职责：
// - 解析 HTTP 请求
// - 调用 service 层执行业务逻辑
// - 构建并返回 HTTP 响应
//
// 按功能分组：
// - 基础端点：health, state, context, intent, api_list
// - 关系端点：relationship list/get/update
// - 寿命端点：lifespan status
// - 记忆端点：memory recent/search/store
// - 验证端点：intent validation

use axum::{
    extract::{Path as AxumPath, State},
    http::{Response, StatusCode},
    response::{IntoResponse, Json},
};
use bytes::Bytes;
use http_body::Frame;
use http_body_util::StreamBody;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{error, info};
use uuid::Uuid;

use crate::ai::cognitive::narrative::NarrativeEngine;
use crate::ai::lifespan::LifespanStatus;
use crate::ai::validator::{PersonaInfo, ValidationRequest, ValidationResult};
use cyber_jianghu_protocol::{ActionType, Intent, ServerMessage};

use super::cognitive_context::{CognitiveContext, CognitiveContextBuilder};
use super::context::{
    ContextResponse, create_attributes_glimpse, generate_context_markdown,
    generate_context_markdown_no_relationship,
};
use super::dto;
use super::dto::{
    HealthResponse, LifespanResponse, RelationshipUpdateRequest, ValidateRequest, ValidateResponse,
};
use super::service::{MemoryService, RelationshipService, memories_to_json_response};
use super::{HttpApiState, IntentRequest};

// ============================================================================
// API 发现端点
// ============================================================================

/// API 端点信息
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiEndpoint {
    /// 端点路径
    pub path: String,
    /// HTTP 方法
    pub method: String,
    /// 描述
    pub description: String,
    /// 请求体示例（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_example: Option<serde_json::Value>,
    /// 响应示例（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_example: Option<serde_json::Value>,
}

/// API 列表响应
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiListResponse {
    /// API 版本
    pub version: String,
    /// Agent ID
    pub agent_id: String,
    /// 可用端点列表
    pub endpoints: Vec<ApiEndpoint>,
}

/// API 列表端点处理器
///
/// GET /api/v1 - 返回所有可用 API 端点和使用说明
pub(super) async fn api_list_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let endpoints = vec![
        // === 基础端点 ===
        ApiEndpoint {
            path: "/api/v1/health".to_string(),
            method: "GET".to_string(),
            description: "健康检查，返回 Agent 状态".to_string(),
            request_example: None,
            response_example: Some(serde_json::json!({
                "status": "ok",
                "agent_id": "uuid-...",
                "tick_id": 123
            })),
        },
        ApiEndpoint {
            path: "/api/v1/state".to_string(),
            method: "GET".to_string(),
            description: "获取当前 WorldState（完整游戏状态）".to_string(),
            request_example: None,
            response_example: Some(serde_json::json!({
                "tick_id": 123,
                "self_state": { "attributes": {}, "inventory": [] },
                "location": { "node_id": "...", "name": "...", "node_type": "..." },
                "entities": [],
                "nearby_items": [],
                "events_log": []
            })),
        },
        ApiEndpoint {
            path: "/api/v1/context".to_string(),
            method: "GET".to_string(),
            description: "获取叙事化上下文（Markdown 格式，推荐用于 LLM）".to_string(),
            request_example: None,
            response_example: Some(serde_json::json!({
                "tick_id": 123,
                "agent_id": "uuid-...",
                "context": "# 游戏状态上下文\n\n## 自身状态\n- 身体: 身体状况极佳..."
            })),
        },
        ApiEndpoint {
            path: "/api/v1/attributes".to_string(),
            method: "GET".to_string(),
            description: "梦中一瞥：获取结构化属性数值（禁止存储到记忆）".to_string(),
            request_example: None,
            response_example: Some(serde_json::json!({
                "tick_id": 123,
                "attributes": [
                    {"name": "hp", "display_name": "生命值", "value_str": "95", "category": "status"}
                ],
                "raw": {"hp": 95, "stamina": 80},
                "warning": "此数据为梦中一瞥，仅限当前决策周期使用..."
            })),
        },
        ApiEndpoint {
            path: "/api/v1/intent".to_string(),
            method: "POST".to_string(),
            description: "提交决策意图".to_string(),
            request_example: Some(serde_json::json!({
                "action_type": "speak",
                "action_data": "大家好！",
                "thought_log": "想和大家打个招呼"
            })),
            response_example: Some(serde_json::json!({
                "status": "submitted"
            })),
        },
        ApiEndpoint {
            path: "/api/v1/cognitive".to_string(),
            method: "GET".to_string(),
            description: "结构化认知上下文（引导 OpenClaw 四阶段推理）".to_string(),
            request_example: None,
            response_example: Some(serde_json::json!({
                "cognitive_context": {
                    "perception": { "self_status": "...", "environment": "...", "key_observations": [] },
                    "motivation": { "active_drives": [], "dominant_drive": "..." },
                    "planning": { "current_goals": [], "available_actions": [] },
                    "decision": { "requires_reasoning": true, "thinking_prompt": "..." }
                },
                "persona": { "name": "柳蕴娘", "personality": [], "description": "..." },
                "world_state": { "attributes": {}, "nearby_entities_count": 0, "time": { "hour": 12, "weather": "晴" } }
            })),
        },
        // === 关系端点 ===
        ApiEndpoint {
            path: "/api/v1/relationship/list".to_string(),
            method: "GET".to_string(),
            description: "获取所有已知关系".to_string(),
            request_example: None,
            response_example: None,
        },
        ApiEndpoint {
            path: "/api/v1/relationship/{id}".to_string(),
            method: "GET".to_string(),
            description: "获取特定关系详情".to_string(),
            request_example: None,
            response_example: None,
        },
        ApiEndpoint {
            path: "/api/v1/relationship".to_string(),
            method: "POST".to_string(),
            description: "更新关系（好感度、事件）".to_string(),
            request_example: Some(serde_json::json!({
                "target_agent_id": "uuid-...",
                "target_name": "李四",
                "favorability_delta": 5,
                "event_type": "chat",
                "event_description": "愉快地交谈"
            })),
            response_example: None,
        },
        // === 寿命端点 ===
        ApiEndpoint {
            path: "/api/v1/lifespan".to_string(),
            method: "GET".to_string(),
            description: "获取寿命状态".to_string(),
            request_example: None,
            response_example: None,
        },
        // === 记忆端点 ===
        ApiEndpoint {
            path: "/api/v1/memory/recent".to_string(),
            method: "GET".to_string(),
            description: "获取近期记忆".to_string(),
            request_example: None,
            response_example: None,
        },
        ApiEndpoint {
            path: "/api/v1/memory/search".to_string(),
            method: "POST".to_string(),
            description: "语义搜索记忆".to_string(),
            request_example: Some(serde_json::json!({
                "query": "战斗",
                "limit": 10
            })),
            response_example: None,
        },
        ApiEndpoint {
            path: "/api/v1/memory".to_string(),
            method: "POST".to_string(),
            description: "存储新记忆".to_string(),
            request_example: Some(serde_json::json!({
                "content": "遇到了一位神秘的老人",
                "importance": 0.8
            })),
            response_example: None,
        },
        // === 验证端点 ===
        ApiEndpoint {
            path: "/api/v1/validate".to_string(),
            method: "POST".to_string(),
            description: "验证意图是否符合人设".to_string(),
            request_example: Some(serde_json::json!({
                "action_type": "attack",
                "action_data": null,
                "persona_gender": "女",
                "persona_age": 25,
                "persona_personality": "温柔善良",
                "persona_values": "和平"
            })),
            response_example: Some(serde_json::json!({
                "valid": true,
                "reason": "动作符合人设",
                "narrative": "..."
            })),
        },
        // === 角色生成端点 ===
        ApiEndpoint {
            path: "/api/v1/character/generate".to_string(),
            method: "POST".to_string(),
            description: "LLM 一键生成角色（返回完整的角色配置，供前端填充表单）".to_string(),
            request_example: None,
            response_example: Some(serde_json::json!({
                "name": "柳蕴娘",
                "age": 24,
                "gender": "女",
                "appearance": "眉如远山，眼若秋水，身着素色罗裙",
                "identity": "江南药铺掌柜之女，自幼随父研习医术",
                "personality": ["沉稳", "善良"],
                "values": ["知识", "和平"],
                "language_style": {
                    "tone": "温和",
                    "speech_patterns": ["说话简洁"]
                },
                "goals": {
                    "short_term": "收集罕见草药配方",
                    "long_term": "编纂一本江湖医术典籍"
                }
            })),
        },
        // === 审查端点（Player Agent 提供，Observer Agent 调用）===
        ApiEndpoint {
            path: "/api/v1/review/pending".to_string(),
            method: "GET".to_string(),
            description: "获取待审查意图列表（Observer Agent 轮询用）".to_string(),
            request_example: None,
            response_example: Some(serde_json::json!({
                "intent_id": "uuid-...",
                "agent_id": "uuid-...",
                "intent": {"action_type": "attack", "action_data": null},
                "persona_summary": {
                    "name": "张三",
                    "gender": "男",
                    "age": 28,
                    "personality": ["沉稳", "重情义"],
                    "values": ["江湖道义为先"]
                },
                "world_context": "当前位置：龙门客栈大堂\n附近实体：李四...",
                "created_at": "2024-03-19T10:00:00Z",
                "deadline": "2024-03-19T10:00:30Z"
            })),
        },
        ApiEndpoint {
            path: "/api/v1/review/{intent_id}".to_string(),
            method: "POST".to_string(),
            description: "提交审查结果（批准/拒绝）".to_string(),
            request_example: Some(serde_json::json!({
                "result": "approved",
                "reason": "行为符合武侠世界观",
                "narrative": "张三决定出手相助"
            })),
            response_example: Some(serde_json::json!({
                "intent_id": "uuid-...",
                "status": "approved",
                "decision": "approved",
                "reason": "行为符合武侠世界观",
                "narrative": "张三决定出手相助",
                "reviewed_at": "2024-03-19T10:00:15Z"
            })),
        },
        ApiEndpoint {
            path: "/api/v1/review/{intent_id}/status".to_string(),
            method: "GET".to_string(),
            description: "获取特定意图的审查状态".to_string(),
            request_example: None,
            response_example: Some(serde_json::json!({
                "intent_id": "uuid-...",
                "status": "approved",
                "decision": "approved",
                "reason": "行为符合武侠世界观",
                "narrative": "张三决定出手相助",
                "reviewed_at": "2024-03-19T10:00:15Z"
            })),
        },
    ];

    let agent_id = *state.agent_id.read().await;
    Json(ApiListResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        agent_id: agent_id.to_string(),
        endpoints,
    })
}

// ============================================================================
// 通用工具方法
// ============================================================================

#[derive(Serialize)]
struct ErrorResponse {
    error_code: String,
    message: String,
}

/// 解析 tick_id：优先使用请求中的值，否则使用当前状态的 tick_id
/// 如果当前没有状态，则拒绝请求
async fn resolve_tick_id_or_reject(
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
pub(super) async fn health_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    let agent_id = *state.agent_id.read().await;

    let response = HealthResponse {
        status: "ok".to_string(),
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
pub(super) async fn get_state_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    match current.as_ref() {
        Some(world_state) => Json(world_state.clone()).into_response(),
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Get formatted context handler (使用叙事化描述，不暴露数值)
pub(super) async fn get_context_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    let agent_id = *state.agent_id.read().await;

    // 获取托梦内容（如果有），每次调用会减少剩余回合数
    let dream_thought = state.consume_dream().await;

    match current.as_ref() {
        Some(world_state) => {
            // 使用叙事引擎生成上下文，不暴露原始数值
            let engine = state
                .narrative_engine
                .as_ref()
                .map(|e| e.as_ref())
                .unwrap_or_else(|| {
                    // 如果没有初始化，使用内置配置创建临时引擎
                    static DEFAULT_ENGINE: std::sync::OnceLock<
                        crate::ai::cognitive::narrative::NarrativeEngine,
                    > = std::sync::OnceLock::new();
                    DEFAULT_ENGINE.get_or_init(
                        crate::ai::cognitive::narrative::NarrativeEngine::with_builtin_config,
                    )
                });

            let context = if let Some(store) = &state.relationship_store {
                generate_context_markdown(world_state, store, engine, dream_thought.as_deref())
            } else {
                generate_context_markdown_no_relationship(
                    world_state,
                    engine,
                    dream_thought.as_deref(),
                )
            };
            Json(ContextResponse {
                context,
                tick_id: world_state.tick_id,
                agent_id: agent_id.to_string(),
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
pub(super) async fn get_attributes_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    match current.as_ref() {
        Some(world_state) => {
            // 使用叙事引擎获取属性显示名称
            let engine = state
                .narrative_engine
                .as_ref()
                .map(|e| e.as_ref())
                .unwrap_or_else(|| {
                    static DEFAULT_ENGINE: std::sync::OnceLock<
                        crate::ai::cognitive::narrative::NarrativeEngine,
                    > = std::sync::OnceLock::new();
                    DEFAULT_ENGINE.get_or_init(
                        crate::ai::cognitive::narrative::NarrativeEngine::with_builtin_config,
                    )
                });

            let glimpse = create_attributes_glimpse(world_state, engine);
            Json(glimpse).into_response()
        }
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Submit intent handler (完全数据驱动)
#[allow(dead_code)]
pub(super) async fn submit_intent_handler(
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
    if let Some(history) = &state.intent_history {
        history
            .record_intent(
                tick_id,
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
// 关系 API Handlers
// ============================================================================

/// 获取所有关系
pub(super) async fn get_relationships_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let store = match &state.relationship_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Relationship store not initialized",
            )
                .into_response();
        }
    };

    let service = RelationshipService::new(store);
    match service.get_all() {
        Ok(relationships) => {
            Json(serde_json::json!({ "relationships": relationships })).into_response()
        }
        Err(e) => {
            error!("[http] Failed to get relationships: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get relationships: {}", e),
            )
                .into_response()
        }
    }
}

/// 获取特定关系
pub(super) async fn get_relationship_handler(
    State(state): State<HttpApiState>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let store = match &state.relationship_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Relationship store not initialized",
            )
                .into_response();
        }
    };

    let target_id = match Uuid::parse_str(&id) {
        Ok(uuid) => uuid,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid UUID format").into_response(),
    };

    let service = RelationshipService::new(store);
    match service.get(target_id) {
        Ok(Some(relationship)) => Json(relationship).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Relationship not found").into_response(),
        Err(e) => {
            error!("[http] Failed to get relationship: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get relationship: {}", e),
            )
                .into_response()
        }
    }
}

/// 更新关系
pub(super) async fn update_relationship_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<RelationshipUpdateRequest>,
) -> impl IntoResponse {
    let store = match &state.relationship_store {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Relationship store not initialized",
            )
                .into_response();
        }
    };

    let target_id = match Uuid::parse_str(&req.target_agent_id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "Invalid target_agent_id format").into_response();
        }
    };

    let tick_id = state
        .current_state
        .read()
        .await
        .as_ref()
        .map(|s| s.tick_id)
        .unwrap_or(0);

    let event = match (&req.event_type, &req.event_description) {
        (Some(event_type), Some(description)) => Some((
            event_type.clone(),
            description.clone(),
            req.event_favorability_delta.unwrap_or(0),
            tick_id,
        )),
        _ => None,
    };

    let service = RelationshipService::new(store);
    match service.update(target_id, &req.target_name, req.favorability_delta, event) {
        Ok(_) => (StatusCode::OK, "Relationship updated").into_response(),
        Err(e) => {
            error!("[http] Failed to update relationship: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to update relationship: {}", e),
            )
                .into_response()
        }
    }
}

// ============================================================================
// 寿命 API Handlers
// ============================================================================

/// 获取寿命状态
pub(super) async fn get_lifespan_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let calculator = match &state.lifespan_calculator {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Lifespan calculator not initialized",
            )
                .into_response();
        }
    };

    let calc = calculator.lock().await;
    let response = match calc.get_status() {
        LifespanStatus::Alive { age } => LifespanResponse {
            current_age: age,
            status: "alive".to_string(),
            aging_effects: None,
        },
        LifespanStatus::Aging { age, effects } => LifespanResponse {
            current_age: age,
            status: "aging".to_string(),
            aging_effects: Some(format!("{:?}", effects)),
        },
        LifespanStatus::Deceased { age } => LifespanResponse {
            current_age: age,
            status: "deceased".to_string(),
            aging_effects: None,
        },
    };
    drop(calc);

    Json(response).into_response()
}

// ============================================================================
// 记忆 API Handlers
// ============================================================================

/// 获取近期记忆
pub(super) async fn get_recent_memory_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let manager = match &state.memory_manager {
        Some(m) => m,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Memory manager not initialized",
            )
                .into_response();
        }
    };

    let mut mgr = manager.lock().await;
    let service = MemoryService::new(&mut mgr);
    let memories = service.get_recent();

    Json(memories_to_json_response(&memories)).into_response()
}

/// 搜索记忆
pub(super) async fn search_memory_handler(
    State(state): State<HttpApiState>,
    Json(request): Json<super::dto::MemorySearchRequest>,
) -> impl IntoResponse {
    let manager = match &state.memory_manager {
        Some(m) => m,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Memory manager not initialized",
            )
                .into_response();
        }
    };

    let mut mgr = manager.lock().await;
    let mut service = MemoryService::new(&mut mgr);
    let limit = request.limit.unwrap_or(10);

    match service.search(&request.query, limit).await {
        Ok(memories) => Json(memories_to_json_response(&memories)).into_response(),
        Err(e) => {
            error!("[http] Failed to search memory: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Search failed: {}", e),
            )
                .into_response()
        }
    }
}

/// 存储记忆
pub(super) async fn store_memory_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<super::dto::MemoryStoreRequest>,
) -> impl IntoResponse {
    let manager = match &state.memory_manager {
        Some(m) => m,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Memory manager not initialized",
            )
                .into_response();
        }
    };

    let tick_id = state
        .current_state
        .read()
        .await
        .as_ref()
        .map(|s| s.tick_id)
        .unwrap_or(0);
    let agent_id = *state.agent_id.read().await;
    let mut mgr = manager.lock().await;
    let mut service = MemoryService::new(&mut mgr);

    match service
        .store(agent_id, tick_id, req.content, req.importance)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({"success": true, "message": "Memory stored"})),
        )
            .into_response(),
        Err(e) => {
            error!("[http] Failed to store memory: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to store memory: {}", e),
            )
                .into_response()
        }
    }
}

// ============================================================================
// 验证 API Handlers
// ============================================================================

/// 验证 Intent（数据驱动）
pub(super) async fn validate_intent_handler(
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
        gender: req.persona_gender.unwrap_or_else(|| "未知".to_string()),
        age: req.persona_age.unwrap_or(28),
        personality: req.persona_personality.unwrap_or_default(),
        values: req.persona_values.unwrap_or_default(),
    };

    let world_state = state.current_state.read().await;
    let world_context = world_state
        .as_ref()
        .map(|ws| format!("Tick: {}, Location: {:?}", ws.tick_id, ws.location))
        .unwrap_or_else(|| "No world state available".to_string());
    drop(world_state);

    let validation_req = ValidationRequest {
        intent,
        persona: persona_info,
        world_context,
    };

    match validator.validate(validation_req).await {
        Ok(ValidationResult::Approved { reason, narrative }) => Json(ValidateResponse {
            valid: true,
            reason,
            rejection_type: None,
            narrative: Some(narrative),
        })
        .into_response(),
        Ok(ValidationResult::Rejected {
            reason,
            rejection_type,
        }) => Json(ValidateResponse {
            valid: false,
            reason: Some(reason),
            rejection_type: Some(rejection_type.as_str().to_string()),
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
// Tick 通知 API Handlers
// ============================================================================

/// 获取当前 Tick 状态
///
/// GET /api/v1/tick - 返回当前 tick 状态，用于轮询检测新 tick
pub(super) async fn get_tick_status_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let current = state.current_state.read().await;
    let agent_id = *state.agent_id.read().await;
    let last_update = state.last_state_update.read().await;

    let (tick_id, has_state, state_tick_id) = match current.as_ref() {
        Some(ws) => (ws.tick_id, true, Some(ws.tick_id)),
        None => (0, false, None),
    };

    let (state_updated_at, state_age_ms) = match *last_update {
        Some(instant) => {
            let age_ms = instant.elapsed().as_millis() as u64;
            // 假设我们不能精确地将 Instant 转为 UTC（因为是单调时钟），
            // 但我们可以用当前 UTC 减去 age_ms 估算。
            let utc_time = chrono::Utc::now() - std::time::Duration::from_millis(age_ms);
            (Some(utc_time.to_rfc3339()), Some(age_ms))
        }
        None => (None, None),
    };

    Json(dto::TickStatusResponse {
        tick_id,
        agent_id: if agent_id.is_nil() {
            None
        } else {
            Some(agent_id.to_string())
        },
        has_new_state: has_state,
        seconds_until_next_tick: None, // 服务端未提供此信息
        last_updated_at: chrono::Utc::now().to_rfc3339(),
        state_tick_id,
        state_updated_at,
        state_age_ms,
    })
}

// ============================================================================
// 角色注册 API Handlers
// ============================================================================

/// 角色注册请求（从 CLI 接收）
#[derive(Debug, Deserialize)]
pub struct CharacterRegisterRequest {
    /// 角色姓名
    pub name: String,
    /// 年龄
    #[serde(default = "default_age")]
    pub age: u8,
    /// 性别
    #[serde(default = "default_gender")]
    pub gender: String,
    /// 外貌描述
    #[serde(default)]
    pub appearance: Option<String>,
    /// 身份背景
    #[serde(default)]
    pub identity: Option<String>,
    /// 性格特征
    #[serde(default)]
    pub personality: Vec<String>,
    /// 核心价值观
    #[serde(default)]
    pub values: Vec<String>,
    /// 语言风格
    #[serde(default)]
    pub language_style: LanguageStyleRequest,
    /// 目标
    #[serde(default)]
    pub goals: GoalsRequest,
    /// 系统提示词（可选）
    #[serde(default)]
    pub system_prompt: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub(super) struct LanguageStyleRequest {
    #[serde(default)]
    tone: Option<String>,
    #[serde(default)]
    speech_patterns: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
pub(super) struct GoalsRequest {
    #[serde(default)]
    short_term: Option<String>,
    #[serde(default)]
    long_term: Option<String>,
}

fn default_age() -> u8 {
    25
}
fn default_gender() -> String {
    "男".to_string()
}

/// 角色注册响应（返回给 CLI）
#[derive(Debug, Serialize)]
pub struct CharacterRegisterResponse {
    /// 角色 ID（服务器分配）
    pub agent_id: String,
    /// 结果消息
    pub message: String,
    /// 警告信息（如配置保存失败）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// LLM 生成角色处理器
///
/// POST /api/v1/character/generate - 使用 LLM 自动生成角色
///
/// 调用配置的 LLM 生成一个符合世界观的武侠角色，返回完整角色信息供用户确认
pub(super) async fn generate_character_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    use crate::ai::llm::{DirectLlmClient, DirectLlmClientConfig, LlmClientExt, LlmProvider};

    // 1. 读取配置文件
    let config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "config_read_error".to_string(),
                    message: format!("读取配置文件失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    // 2. 检查 LLM 是否已配置
    if config.llm.model.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error_code: "llm_not_configured".to_string(),
                message: "请先配置 LLM".to_string(),
            }),
        )
            .into_response();
    }

    // 3. 创建 LLM 客户端
    let provider = match LlmProvider::parse(&config.llm.provider) {
        Some(p) => p,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "invalid_provider".to_string(),
                    message: format!("不支持的 LLM Provider: {}", config.llm.provider),
                }),
            )
                .into_response();
        }
    };

    let mut client_config = DirectLlmClientConfig::new(provider, config.llm.api_key.as_deref());

    if let Some(ref model) = config.llm.model {
        client_config = client_config.with_model(model);
    }
    if let Some(ref base_url) = config.llm.base_url {
        client_config = client_config.with_base_url(base_url);
    }
    client_config = client_config.with_temperature(0.9);

    let llm_client = match DirectLlmClient::new(client_config) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "llm_client_error".to_string(),
                    message: format!("创建 LLM 客户端失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    // 4. 构建角色生成 prompt
    let prompt = r#"你是一个武侠角色生成器。请生成一个符合以下世界观的角色：

## 世界观
时代：北宋前期（约10世纪中国），冷兵器时代。
允许的概念：内力、轻功、武功、点穴，暗器、毒术、医术、易容、阵法。
禁止的概念：魔法、仙术、法术、热武器、现代科技、超能力、穿越。

## 核心要求
1. **多样性**：生成的每个角色必须在姓名、身份背景、性格、价值观、语言风格、目标等方面与常见角色有明显差异
2. **避免重复**：不要生成重复或相似的角色，不同角色应该有截然不同的背景故事和个性
3. **真实性**：角色应该像一个真实的人，有复杂的动机和独特的说话方式

## 字段要求
- name: 姓名（2-6个汉字）
- age: 年龄（16-60的整数）
- gender: 性别（"男"或"女"）
- appearance: 外貌描述（20-50字），要有特色
- identity: 身份背景（如"江湖游侠"、"药铺掌柜"，不超过300字），要有独特的故事
- personality: 性格特征数组（从以下选项中选2-4个：豪爽、沉稳、机智、冷漠、善良、阴险、正义、贪婪、忠诚、狡猾），避免只选正面或只选负面，性格特征需要与身份背景吻合。
- values: 核心价值观数组（从以下选项中选1-3个：侠义、财富、权力、自由、荣誉、知识、爱情、友情、复仇、和平），核心价值观需要与身份背景吻合。
- language_style: 对象，包含：
  - tone: 语调（从以下选项中选1个：豪迈、温和、冷漠、狡黠、文雅）
  - speech_patterns: 说话特点数组（从以下选项中选1-3个：喜欢引用古诗词、说话简洁、喜欢用成语、说话带方言、喜欢开玩笑、说话谨慎）
- goals: 对象，包含：
  - short_term: 短期目标（不超过100字），要具体且有个人特色
  - long_term: 长远目标（不超过100字），要有野心或深度

## 输出格式
请严格输出 JSON，不要包含其他文字。"#;

    // 5. 调用 LLM 生成角色
    #[derive(Debug, Serialize, Deserialize)]
    struct GeneratedCharacter {
        name: String,
        age: u8,
        gender: String,
        appearance: Option<String>,
        identity: Option<String>,
        personality: Vec<String>,
        values: Vec<String>,
        language_style: LanguageStyleRequest,
        goals: GoalsRequest,
    }

    match llm_client.complete_json::<GeneratedCharacter>(prompt).await {
        Ok(character) => {
            info!("[character] LLM 生成角色成功: {}", character.name);
            (StatusCode::OK, Json(character)).into_response()
        }
        Err(e) => {
            error!("[character] LLM 生成角色失败: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error_code: "generation_failed".to_string(),
                    message: format!("角色生成失败，请重试: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// 角色注册处理器
///
/// POST /api/v1/character/register - 创建新角色
///
/// 接收 CLI 的角色创建请求，添加设备认证信息后转发到 Server
pub(super) async fn register_character_handler(
    State(state): State<HttpApiState>,
    Json(payload): Json<CharacterRegisterRequest>,
) -> impl IntoResponse {
    use reqwest::Client;
    use tracing::info;

    // 1. 检查设备身份
    let identity = match &state.identity {
        Some(id) => id,
        None => {
            error!("设备身份未初始化，请先启动 Agent 进行设备注册");
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(CharacterRegisterResponse {
                    agent_id: String::new(),
                    message: "设备身份未初始化，请先启动 Agent".to_string(),
                    warning: None,
                }),
            )
                .into_response();
        }
    };

    info!("角色注册请求: {}", payload.name);

    // 2. 生成默认 system_prompt（如果未提供）
    let system_prompt = payload.system_prompt.clone().unwrap_or_else(|| {
        format!(
            "你是{}，{}，{}岁。{}{}你的目标是探索这个江湖世界，与各路侠客交流，并在武林中闯出自己的一片天地。",
            payload.name,
            payload.identity.as_deref().unwrap_or("江湖中人"),
            payload.age,
            payload.appearance.as_deref().map(|a| format!("{}。", a)).unwrap_or_default(),
            if !payload.personality.is_empty() {
                format!("性格特点：{}。", payload.personality.join("、"))
            } else {
                String::new()
            }
        )
    });

    // 3. 构建发送到 Server 的请求
    let server_request = serde_json::json!({
        "device_id": identity.device_id,
        "auth_token": identity.auth_token,
        "name": payload.name,
        "age": payload.age,
        "gender": payload.gender,
        "appearance": payload.appearance,
        "identity": payload.identity,
        "personality": payload.personality,
        "values": payload.values,
        "language_style": payload.language_style,
        "goals": payload.goals,
        "system_prompt": system_prompt,
    });

    // 4. 转发到 Server
    let client = Client::new();
    let server_http_url = state.server_http_url.read().await.clone();
    let server_url = format!("{}/api/v1/agent/register", server_http_url);

    let response = match client.post(&server_url).json(&server_request).send().await {
        Ok(resp) => resp,
        Err(e) => {
            error!("连接服务器失败: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(CharacterRegisterResponse {
                    agent_id: String::new(),
                    message: format!("连接服务器失败: {}", e),
                    warning: None,
                }),
            )
                .into_response();
        }
    };

    // 5. 处理 Server 响应
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        error!("服务器拒绝注册: {} - {}", status, body);
        return (
            status,
            Json(CharacterRegisterResponse {
                agent_id: String::new(),
                message: format!("服务器拒绝: {}", body),
                warning: None,
            }),
        )
            .into_response();
    }

    // 6. 解析成功响应
    #[derive(Deserialize)]
    struct ServerRegisterResponse {
        agent_id: String,
        message: String,
        game_rules: Option<cyber_jianghu_protocol::GameRules>,
        narrative_config: Option<cyber_jianghu_protocol::NarrativeConfig>,
        #[serde(default)]
        initial_attributes: std::collections::HashMap<String, i32>,
    }

    match response.json::<ServerRegisterResponse>().await {
        Ok(result) => {
            info!("角色注册成功: {} -> {}", payload.name, result.agent_id);

            // 7. 保存 narrative_config 到本地配置目录
            if let Some(ref narrative_config) = result.narrative_config
                && let Some(home) = dirs::home_dir()
            {
                let config_dir = home.join(".cyber-jianghu").join("config");
                if let Err(e) = std::fs::create_dir_all(&config_dir) {
                    error!("创建配置目录失败: {}", e);
                } else {
                    let config_path = config_dir.join("narrative_config.json");
                    match serde_json::to_string_pretty(narrative_config) {
                        Ok(json) => {
                            if let Err(e) = std::fs::write(&config_path, json) {
                                error!("保存 narrative_config 失败: {}", e);
                            } else {
                                info!("已保存 narrative_config 到 {:?}", config_path);
                            }
                        }
                        Err(e) => error!("序列化 narrative_config 失败: {}", e),
                    }
                }
            }

            // 8. 更新本地配置文件（添加 agent_id、注册时间、先天属性和游戏规则）
            let mut config_warning = None;
            if let Ok(mut config) = crate::config::Config::from_file(&state.config_path) {
                if let Some(ref game_rules) = result.game_rules {
                    config.update_game_rules(game_rules.clone());
                }

                // 如果 agent 不存在，创建新的 CharacterConfig
                if config.agent.is_none() {
                    config.agent = Some(crate::config::CharacterConfig {
                        name: payload.name.clone(),
                        age: payload.age,
                        gender: payload.gender.clone(),
                        appearance: payload.appearance.clone(),
                        identity: payload.identity.clone(),
                        personality: payload.personality.clone(),
                        values: payload.values.clone(),
                        language_style: crate::config::LanguageStyleConfig {
                            tone: payload.language_style.tone.clone(),
                            speech_patterns: payload.language_style.speech_patterns.clone(),
                        },
                        goals: crate::config::GoalsConfig {
                            short_term: payload.goals.short_term.clone(),
                            long_term: payload.goals.long_term.clone(),
                        },
                        system_prompt: Some(system_prompt.clone()),
                        ..Default::default()
                    });
                }

                if let Some(ref mut agent) = config.agent {
                    // 保存服务器返回的 agent_id
                    if let Ok(agent_uuid) = uuid::Uuid::parse_str(&result.agent_id) {
                        agent.agent_id = Some(agent_uuid);
                    }
                    agent.registered_at = Some(chrono::Utc::now());
                    // 保存先天属性（只保存先天属性，不包含状态值）
                    if !result.initial_attributes.is_empty() {
                        agent.birth_attributes = Some(result.initial_attributes.clone());
                    }
                    // 写入 characters 数组（供世界树展示历史角色）
                    let char_clone = agent.clone();
                    config.upsert_character(char_clone);
                }
                if let Err(e) = config.save_to_file(&state.config_path) {
                    error!("保存配置文件失败: {}", e);
                    config_warning = Some(format!("配置保存失败: {}", e));
                }
            }

            // 9. 更新运行时 agent_id（使后续 Intent 提交使用新角色）
            if let Ok(agent_uuid) = uuid::Uuid::parse_str(&result.agent_id) {
                let mut id = state.agent_id.write().await;
                *id = agent_uuid;
                info!(
                    "[character] Updated runtime agent_id to {} ({})",
                    agent_uuid, payload.name
                );
            }

            (
                StatusCode::OK,
                Json(CharacterRegisterResponse {
                    agent_id: result.agent_id,
                    message: result.message,
                    warning: config_warning,
                }),
            )
                .into_response()
        }
        Err(e) => {
            error!("解析服务器响应失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CharacterRegisterResponse {
                    agent_id: String::new(),
                    message: format!("解析响应失败: {}", e),
                    warning: None,
                }),
            )
                .into_response()
        }
    }
}

// ============================================================================
// 角色信息 API Handlers
// ============================================================================

/// 角色信息响应（合并配置文件 + WorldState 实时数据）
#[derive(Debug, Serialize)]
pub struct CharacterInfoResponse {
    // === 配置文件数据（注册时提供） ===
    /// 角色 ID
    pub agent_id: Option<String>,
    /// 姓名
    pub name: String,
    /// 年龄
    pub age: u8,
    /// 性别
    pub gender: String,
    /// 外貌描述
    pub appearance: Option<String>,
    /// 身份背景
    pub identity: Option<String>,
    /// 性格特征
    pub personality: Vec<String>,
    /// 核心价值观
    pub values: Vec<String>,

    // === 注册信息 ===
    /// 注册时间（ISO 8601 格式）
    pub registered_at: Option<String>,

    // === WorldState 实时数据 ===
    /// 当前属性（带叙事描述）
    pub attributes: Option<serde_json::Value>,
    /// 派生属性（带叙事描述，浮点值）
    pub derived_attributes: Option<serde_json::Value>,
    /// 先天属性（注册时的属性值）
    pub birth_attributes: Option<serde_json::Value>,
    /// 持有物品
    pub inventory: Option<serde_json::Value>,
    /// 当前位置
    pub location: Option<String>,
    /// 当前 Tick
    pub tick_id: Option<i64>,
    /// 游戏时间
    pub world_time: Option<serde_json::Value>,

    // === 状态 ===
    /// 角色状态（alive, dead, etc.）
    pub status: Option<String>,
    /// 数据是否来自缓存（true = 数据可能已过时）
    pub is_stale: bool,
}

/// 获取角色信息
///
/// GET /api/v1/character - 获取当前角色完整信息
///
/// 数据来源：
/// - 配置文件：name, age, gender, appearance, identity, personality, values
/// - WorldState：attributes, inventory, location, tick_id, world_time
pub(super) async fn get_character_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    // 1. 从配置文件读取角色配置
    let config = match crate::config::Config::from_file(&state.config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            error!("读取配置文件失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "config_read_error".to_string(),
                    message: format!("读取配置文件失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    // 2. 从配置中提取角色信息
    let character = match &config.agent {
        Some(ch) => ch,
        None => {
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(ErrorResponse {
                    error_code: "character_not_registered".to_string(),
                    message: "角色尚未注册，请先创建角色".to_string(),
                }),
            )
                .into_response();
        }
    };

    // 3. 加载叙事配置（用于属性描述）
    let narrative_config = state.narrative_config.read().await.clone();

    // 4. 从当前 WorldState 获取实时状态
    let current = state.current_state.read().await;

    // 是否使用缓存数据（当服务器未连接时）
    let is_stale = current.is_none();

    let (agent_id, raw_attributes, raw_derived, inventory, location, tick_id, world_time) =
        match current.as_ref() {
            Some(ws) => {
                let agent_id = ws.agent_id.map(|id| id.to_string());
                let attrs = serde_json::to_value(&ws.self_state.attributes).ok();
                let derived = serde_json::to_value(&ws.self_state.derived_attributes).ok();
                let inv = serde_json::to_value(&ws.self_state.inventory).ok();
                let loc = Some(format!("{} ({})", ws.location.name, ws.location.node_type));
                let time = serde_json::to_value(&ws.world_time).ok();
                (agent_id, attrs, derived, inv, loc, Some(ws.tick_id), time)
            }
            None => {
                let fallback_attrs = character
                    .birth_attributes
                    .as_ref()
                    .and_then(|a| serde_json::to_value(a).ok());
                (
                    character.agent_id.map(|id| id.to_string()),
                    fallback_attrs,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            }
        };

    // 5. 计算角色状态（在 move attributes 之前）
    // 优先使用 is_dead 标志（当 AgentDied 消息已收到但 WorldState 尚未更新时）
    let status = if state.is_dead.load(std::sync::atomic::Ordering::Relaxed) {
        Some("dead".to_string())
    } else {
        raw_attributes
            .as_ref()
            .and_then(|a| a.get("hp"))
            .and_then(|hp| hp.as_i64())
            .map(|hp| if hp > 0 { "alive" } else { "dead" }.to_string())
    };

    // 6. 丰富属性数据（添加叙事描述）
    let attributes = enrich_attributes_with_descriptions(raw_attributes, &narrative_config);
    let derived_attributes =
        enrich_derived_attributes(raw_derived, &narrative_config);

    // 7. 构建响应
    let response = CharacterInfoResponse {
        agent_id,
        name: character.name.clone(),
        age: character.age,
        gender: character.gender.clone(),
        appearance: character.appearance.clone(),
        identity: character.identity.clone(),
        personality: character.personality.clone(),
        values: character.values.clone(),
        registered_at: character.registered_at.map(|t| t.to_rfc3339()),
        attributes,
        derived_attributes,
        birth_attributes: character
            .birth_attributes
            .as_ref()
            .and_then(|a| serde_json::to_value(a).ok()),
        inventory,
        location,
        tick_id,
        world_time,
        status,
        is_stale,
    };

    Json(response).into_response()
}

/// 属性元数据响应
#[derive(Debug, Serialize)]
pub struct AttributeMetaResponse {
    /// 属性分类
    pub categories: HashMap<String, Vec<String>>,
}

pub(super) async fn get_attribute_meta_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let categories = state
        .narrative_config
        .read()
        .await
        .as_ref()
        .map(|c| c.attribute_categories.clone())
        .unwrap_or_default();
    Json(AttributeMetaResponse { categories }).into_response()
}

/// 丰富属性数据，添加叙事描述
///
/// 从服务器返回的原始属性中：
/// - 提取 `{key}_max` 字段作为属性最大值（服务器通过 max_value_formula 计算）
/// - 如果没有 `{key}_max` 字段，说明该属性没有上限（如声望、派生属性）
fn enrich_attributes_with_descriptions(
    raw_attributes: Option<serde_json::Value>,
    narrative_config: &Option<crate::ai::cognitive::narrative::NarrativeConfig>,
) -> Option<serde_json::Value> {
    let attrs = raw_attributes?;
    let attrs_obj = attrs.as_object()?;

    // 预先收集所有 _max 字段
    let max_values: std::collections::HashMap<&str, i64> = attrs_obj
        .iter()
        .filter_map(|(key, value)| {
            key.strip_suffix("_max")
                .and_then(|base| value.as_i64().map(|v| (base, v)))
        })
        .collect();

    // 将属性转换为带描述的格式（排除 _max 冗余字段）
    let enriched: serde_json::Map<String, serde_json::Value> = attrs_obj
        .iter()
        .filter(|(key, _)| !key.ends_with("_max")) // 排除 _max 字段
        .filter_map(|(key, value)| {
            // 获取当前值
            let current = match value.as_i64() {
                Some(v) => v,
                None => return None,
            };

            // 从叙事配置获取属性信息
            let (display_name, description) = narrative_config
                .as_ref()
                .and_then(|cfg| cfg.attributes.get(key))
                .map(|attr_cfg| {
                    let name = attr_cfg.display_name.clone();
                    let current_i32 = current as i32;
                    let desc = attr_cfg
                        .thresholds
                        .iter()
                        .rev()
                        .find(|t| current_i32 >= t.min && current_i32 <= t.max)
                        .map(|t| t.description.clone())
                        .unwrap_or_else(|| format!("{}: {}", name, current));
                    (name, desc)
                })
                .unwrap_or_else(|| (key.clone(), format!("{}: {}", key, current)));

            // 从服务器返回的 {key}_max 字段获取最大值
            // 如果没有 _max 字段，说明该属性没有上限（如声望、派生属性）
            let max = max_values.get(key.as_str()).copied();

            // 构建属性对象
            let attr_obj = if let Some(max_val) = max {
                serde_json::json!({
                    "name": display_name,
                    "current": current,
                    "max": max_val,
                    "description": description
                })
            } else {
                // 没有上限的属性，不设置 max 字段
                serde_json::json!({
                    "name": display_name,
                    "current": current,
                    "description": description
                })
            };

            Some((key.clone(), attr_obj))
        })
        .collect();

    Some(serde_json::Value::Object(enriched))
}

fn enrich_derived_attributes(
    raw_derived: Option<serde_json::Value>,
    narrative_config: &Option<crate::ai::cognitive::narrative::NarrativeConfig>,
) -> Option<serde_json::Value> {
    let derived = raw_derived?;
    let derived_obj = derived.as_object()?;

    let enriched: serde_json::Map<String, serde_json::Value> = derived_obj
        .iter()
        .filter_map(|(key, value)| {
            let current = match value.as_f64() {
                Some(v) => v,
                None => return None,
            };

            let is_rate = key.ends_with("_rate") || key.ends_with("_bonus");
            let display_current = if is_rate {
                format!("{:.2}%", current * 100.0)
            } else {
                format!("{:.2}", current)
            };

            let (display_name, description) = narrative_config
                .as_ref()
                .and_then(|cfg| cfg.attributes.get(key))
                .map(|attr_cfg| {
                    let name = attr_cfg.display_name.clone();
                    let desc = format!("{}: {}", name, display_current);
                    (name, desc)
                })
                .unwrap_or_else(|| (key.clone(), format!("{}: {}", key, display_current)));

            let attr_obj = serde_json::json!({
                "name": display_name,
                "current": display_current,
                "description": description
            });

            Some((key.clone(), attr_obj))
        })
        .collect();

    Some(serde_json::Value::Object(enriched))
}

/// 经历日志条目
#[derive(Debug, Clone, Serialize)]
pub struct ExperienceEntry {
    /// Tick ID
    pub tick_id: i64,
    /// 游戏时间
    pub world_time: Option<serde_json::Value>,
    /// 现实时间（RFC3339）
    pub created_at: String,
    /// 事件描述
    pub event: String,
    /// 动作类型
    pub action_type: Option<String>,
    /// 观察者思维链（可选）
    pub observer_thought: Option<String>,
    /// 意图摘要（可选）
    pub intent_summary: Option<String>,
}

/// 经历日志响应
#[derive(Debug, Serialize)]
pub struct ExperiencesResponse {
    /// 当前页
    pub page: u32,
    /// 每页数量
    pub limit: u32,
    /// 总数
    pub total: u32,
    /// 是否有更多
    pub has_more: bool,
    /// 经历列表
    pub experiences: Vec<ExperienceEntry>,
}

/// 获取经历日志（分页）
///
/// GET /api/v1/character/experiences?page=1&limit=20
///
/// 数据来源：IntentHistoryStore（SQLite 持久化，按角色隔离）
/// - event: WorldState.events_log 中的事件描述
/// - observer_thought: Observer Agent 审查时的思维链
/// - intent_summary: Agent 提交 Intent 时的 thought_log
pub(super) async fn get_experiences_handler(
    State(state): State<HttpApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let page: u32 = params.get("page").and_then(|s| s.parse().ok()).unwrap_or(1);
    let limit: u32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    let (entries, total) = match &state.intent_history {
        Some(history) => match history.get_page(page, limit).await {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!("[experiences] Failed to query intent history: {}", e);
                (vec![], 0)
            }
        },
        None => (vec![], 0),
    };

    let experiences: Vec<ExperienceEntry> = entries
        .into_iter()
        .map(|e| {
            let world_time = e.world_time.and_then(|s| serde_json::from_str(&s).ok());
            ExperienceEntry {
                tick_id: e.tick_id,
                world_time,
                created_at: e.created_at.to_rfc3339(),
                event: e.event.unwrap_or_default(),
                action_type: Some(e.action_type).filter(|s| !s.is_empty()),
                observer_thought: e.observer_thought,
                intent_summary: e.thought_log,
            }
        })
        .collect();

    let has_more = (page * limit) < total;

    Json(ExperiencesResponse {
        page,
        limit,
        total,
        has_more,
        experiences,
    })
}

/// 转生请求
#[derive(Debug, Deserialize)]
pub struct RebirthRequest {
    /// 确认转生
    pub confirm: bool,
}

/// 转生响应
#[derive(Debug, Serialize)]
pub struct RebirthResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
}

/// 转生（强制归隐重新注册）
///
/// POST /api/v1/character/rebirth
///
/// 删除当前角色，保留设备身份，允许重新创建新角色
pub(super) async fn rebirth_character_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<RebirthRequest>,
) -> impl IntoResponse {
    use tracing::info;

    if !req.confirm {
        return (
            StatusCode::BAD_REQUEST,
            Json(RebirthResponse {
                success: false,
                message: "请确认转生操作 (confirm: true)".to_string(),
            }),
        )
            .into_response();
    }

    // 1. 检查设备身份
    let identity = match &state.identity {
        Some(id) => id,
        None => {
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(RebirthResponse {
                    success: false,
                    message: "设备身份未初始化".to_string(),
                }),
            )
                .into_response();
        }
    };

    // 2. 获取当前 agent_id
    let agent_id = *state.agent_id.read().await;
    if agent_id.is_nil() {
        return (
            StatusCode::PRECONDITION_FAILED,
            Json(RebirthResponse {
                success: false,
                message: "当前没有已注册的角色".to_string(),
            }),
        )
            .into_response();
    }

    info!("角色转生: agent_id={}", agent_id);

    // 3. 通知 Server 删除角色（POST /api/v1/agent/rebirth）
    let client = reqwest::Client::new();
    let server_http_url = state.server_http_url.read().await.clone();
    let server_url = format!("{}/api/v1/agent/rebirth", server_http_url);

    // 构造请求体
    #[derive(Serialize)]
    struct ServerRebirthRequest {
        device_id: Uuid,
        auth_token: String,
    }

    let request_body = ServerRebirthRequest {
        device_id: identity.device_id,
        auth_token: identity.auth_token.clone(),
    };

    let response = match client.post(&server_url).json(&request_body).send().await {
        Ok(resp) => resp,
        Err(e) => {
            error!("连接服务器失败: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(RebirthResponse {
                    success: false,
                    message: format!("连接服务器失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let skip_retire = body.contains("没有活跃的角色") || body.contains("无需归隐");

    if !status.is_success() && !skip_retire {
        error!("服务器转生请求失败: {} - {}", status, body);
        return (
            StatusCode::BAD_GATEWAY,
            Json(RebirthResponse {
                success: false,
                message: format!("服务器拒绝转生: {}", body),
            }),
        )
            .into_response();
    }
    let is_dead_character = skip_retire;

    {
        let mut agent_id_guard = state.agent_id.write().await;
        *agent_id_guard = Uuid::nil();
    }

    // 6. 清理本地 WorldState
    {
        let mut current = state.current_state.write().await;
        *current = None;
    }

    // 7. 更新本地配置：区分死亡角色和正常归隐
    {
        let agent_config = &state.config_path;
        if agent_config.exists() {
            match crate::config::Config::from_file(agent_config) {
                Ok(mut config) => {
                    // 死亡/归隐角色都需要写入 characters 数组（供世界树展示）
                    if is_dead_character {
                        // 死亡角色：标记为 dead 并写入 characters 数组
                        if let Some(ref mut agent) = config.agent {
                            agent.status = crate::config::CharacterStatus::Dead;
                        }
                        if let Some(agent) = config.agent.clone() {
                            config.upsert_character(agent);
                        }
                    } else {
                        config.retire_current_character();
                    }
                    config.agent = None;
                    if let Err(e) = config.save_to_file(agent_config) {
                        error!("保存配置文件失败: {}", e);
                    } else {
                        if is_dead_character {
                            info!("死亡角色已清理本地状态，配置已更新: {:?}", agent_config);
                        } else {
                            info!("角色已归隐，配置已更新: {:?}", agent_config);
                        }
                    }
                }
                Err(e) => {
                    error!("读取配置文件失败: {}", e);
                }
            }
        }
    }

    // 8. 触发重连，让主循环重新注册新角色
    if let Some(ref tx) = state.reconnect_tx {
        let server_ws_url = state.server_ws_url.read().await.clone();
        let reconnect_req = super::ReconnectRequest {
            ws_url: server_ws_url,
        };
        if let Err(e) = tx.send(reconnect_req).await {
            error!("发送重连请求失败: {}", e);
        } else {
            info!("转生后触发 WebSocket 重连");
        }
    }

    Json(RebirthResponse {
        success: true,
        message: "转生成功，请重新创建角色".to_string(),
    })
    .into_response()
}

/// 托梦请求
#[derive(Debug, Deserialize)]
pub struct DreamRequest {
    /// 念头内容（注入到上下文）
    pub thought: String,
    /// 持续回合数
    #[serde(default = "default_dream_duration")]
    pub duration: u32,
}

fn default_dream_duration() -> u32 {
    5
}

/// 托梦响应
#[derive(Debug, Serialize)]
pub struct DreamResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
    /// 剩余回合数
    pub remaining_ticks: u32,
    /// 今天是否还能使用
    pub can_use_today: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamRecord {
    pub injected_at: String,
    pub thought: String,
    pub duration: u32,
}

/// 托梦状态（存储在 HttpApiState 中）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DreamState {
    /// 当前托梦内容
    pub thought: Option<String>,
    /// 剩余回合数
    pub remaining_ticks: u32,
    pub records: Vec<DreamRecord>,
    /// 上次使用的游戏日期（用于每日限制）
    pub last_used_game_date: Option<GameDate>,
    #[serde(skip)]
    pub loaded: bool,
    #[serde(skip)]
    pub current_agent_id: Option<uuid::Uuid>,
}

impl DreamState {
    pub fn load_from_file(config_path: &std::path::Path, agent_id: &uuid::Uuid) -> Option<Self> {
        if agent_id.is_nil() {
            return None;
        }
        if let Some(dir) = config_path.parent() {
            let file_path = dir.join(format!("dream_state_{}.json", agent_id));
            if file_path.exists() {
                match std::fs::read_to_string(&file_path) {
                    Ok(content) => match serde_json::from_str::<Self>(&content) {
                        Ok(mut state) => {
                            state.loaded = true;
                            state.current_agent_id = Some(*agent_id);
                            return Some(state);
                        }
                        Err(e) => {
                            tracing::error!("反序列化托梦记录失败 {:?}: {}", file_path, e);
                        }
                    },
                    Err(e) => {
                        tracing::error!("读取托梦记录文件失败 {:?}: {}", file_path, e);
                    }
                }
            }
        }
        None
    }

    pub fn save_to_file(&self, config_path: &std::path::Path, agent_id: &uuid::Uuid) {
        if agent_id.is_nil() {
            return;
        }
        if let Some(dir) = config_path.parent() {
            let file_path = dir.join(format!("dream_state_{}.json", agent_id));
            match serde_json::to_string_pretty(self) {
                Ok(json) => {
                    if let Err(e) = std::fs::write(&file_path, json) {
                        tracing::error!("写入托梦记录文件失败 {:?}: {}", file_path, e);
                    }
                }
                Err(e) => {
                    tracing::error!("序列化托梦记录失败: {}", e);
                }
            }
        }
    }

    pub fn ensure_loaded(&mut self, config_path: &std::path::Path, agent_id: &uuid::Uuid) {
        if agent_id.is_nil() {
            return;
        }
        if self.loaded && self.current_agent_id == Some(*agent_id) {
            return;
        }
        if let Some(loaded) = Self::load_from_file(config_path, agent_id) {
            *self = loaded;
        } else {
            self.thought = None;
            self.remaining_ticks = 0;
            self.records.clear();
            self.last_used_game_date = None;
            self.loaded = true;
            self.current_agent_id = Some(*agent_id);
        }
    }
}

/// 游戏日期（用于每日限制）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameDate {
    pub year: i32,
    pub month: i32,
    pub day: i32,
}

impl GameDate {
    pub fn from_world_time(world_time: &cyber_jianghu_protocol::WorldTime) -> Self {
        Self {
            year: world_time.year,
            month: world_time.month,
            day: world_time.day,
        }
    }
}

/// 托梦（持续 n 回合的念头注入）
///
/// POST /api/v1/character/dream
///
/// 将念头注入到 Agent 的上下文中，持续指定回合数
pub(super) async fn dream_character_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<DreamRequest>,
) -> impl IntoResponse {
    use tracing::info;

    // 检查是否有托梦存储
    let dream_store = match &state.dream_store {
        Some(store) => store,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(DreamResponse {
                    success: false,
                    message: "托梦功能未初始化".to_string(),
                    remaining_ticks: 0,
                    can_use_today: false,
                }),
            )
                .into_response();
        }
    };

    // 获取当前 WorldState
    let current = state.current_state.read().await;
    let ws = match current.as_ref() {
        Some(ws) => ws,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(DreamResponse {
                    success: false,
                    message: "游戏状态尚未加载".to_string(),
                    remaining_ticks: 0,
                    can_use_today: false,
                }),
            )
                .into_response();
        }
    };

    let current_date = GameDate::from_world_time(&ws.world_time);

    // 检查每日限制
    {
        let mut dream = dream_store.write().await;
        let agent_id = *state.agent_id.read().await;
        dream.ensure_loaded(&state.config_path, &agent_id);

        if let Some(ref last_date) = dream.last_used_game_date
            && last_date == &current_date
        {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(DreamResponse {
                    success: false,
                    message: "今日已使用过托梦，请明天再试".to_string(),
                    remaining_ticks: dream.remaining_ticks,
                    can_use_today: false,
                }),
            )
                .into_response();
        }
    }

    info!(
        "托梦注入: thought={}, duration={}, game_date={}-{}-{}",
        req.thought, req.duration, current_date.year, current_date.month, current_date.day
    );

    // 更新托梦状态
    let mut dream = dream_store.write().await;
    let agent_id = *state.agent_id.read().await;
    dream.ensure_loaded(&state.config_path, &agent_id);

    dream.thought = Some(req.thought.clone());
    dream.remaining_ticks = req.duration;
    dream.last_used_game_date = Some(current_date);
    dream.records.insert(
        0,
        DreamRecord {
            injected_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            thought: req.thought.clone(),
            duration: req.duration,
        },
    );
    if dream.records.len() > 200 {
        dream.records.truncate(200);
    }
    dream.save_to_file(&state.config_path, &agent_id);

    Json(DreamResponse {
        success: true,
        message: format!("托梦成功，将持续 {} 回合", req.duration),
        remaining_ticks: req.duration,
        can_use_today: false, // 刚用过，今天不能再用了
    })
    .into_response()
}

/// 获取当前托梦状态
///
/// GET /api/v1/character/dream
pub(super) async fn get_dream_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let dream_store = match &state.dream_store {
        Some(store) => store,
        None => {
            return Json(DreamStatusResponse {
                thought: None,
                remaining_ticks: 0,
                can_use_today: true,
            })
            .into_response();
        }
    };

    let mut dream = dream_store.write().await;
    let agent_id = *state.agent_id.read().await;
    dream.ensure_loaded(&state.config_path, &agent_id);

    // 获取当前游戏日期，判断今天是否还能使用
    let can_use_today = {
        let current = state.current_state.read().await;
        match current.as_ref() {
            Some(ws) => {
                let current_date = GameDate::from_world_time(&ws.world_time);
                dream.last_used_game_date.as_ref() != Some(&current_date)
            }
            None => true, // 没有状态时默认可用
        }
    };

    Json(DreamStatusResponse {
        thought: dream.thought.clone(),
        remaining_ticks: dream.remaining_ticks,
        can_use_today,
    })
    .into_response()
}

/// 托梦状态响应
#[derive(Debug, Serialize)]
pub struct DreamStatusResponse {
    /// 当前托梦内容
    pub thought: Option<String>,
    /// 剩余回合数
    pub remaining_ticks: u32,
    /// 今天是否还能使用
    pub can_use_today: bool,
}

#[derive(Debug, Serialize)]
pub struct DreamRecordsResponse {
    pub page: u32,
    pub limit: u32,
    pub total: u32,
    pub has_more: bool,
    pub records: Vec<DreamRecord>,
}

pub(super) async fn get_dream_records_handler(
    State(state): State<HttpApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let page: u32 = params.get("page").and_then(|s| s.parse().ok()).unwrap_or(1);
    let limit: u32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    let Some(dream_store) = &state.dream_store else {
        return Json(DreamRecordsResponse {
            page,
            limit,
            total: 0,
            has_more: false,
            records: vec![],
        })
        .into_response();
    };

    let mut dream = dream_store.write().await;
    let agent_id = *state.agent_id.read().await;
    dream.ensure_loaded(&state.config_path, &agent_id);

    let total = dream.records.len() as u32;
    let start = ((page - 1) * limit) as usize;
    let end = std::cmp::min(start + limit as usize, dream.records.len());
    let records = if start < dream.records.len() {
        dream.records[start..end].to_vec()
    } else {
        vec![]
    };

    Json(DreamRecordsResponse {
        page,
        limit,
        total,
        has_more: end < dream.records.len(),
        records,
    })
    .into_response()
}

// ============================================================================
// 多角色管理 API Handlers
// ============================================================================

/// 角色列表响应
#[derive(Debug, Serialize)]
pub struct CharacterListResponse {
    /// 所有角色列表
    pub characters: Vec<CharacterInfo>,
    /// 当前活跃角色的 agent_id
    pub current_agent_id: Option<String>,
    /// 当前服务器 HTTP URL
    pub current_server_url: String,
}

/// 角色详细信息（用于列表展示）
#[derive(Debug, Serialize)]
pub struct CharacterInfo {
    /// 角色 ID
    pub agent_id: Option<String>,
    /// 姓名
    pub name: String,
    /// 年龄
    pub age: u8,
    /// 性别
    pub gender: String,
    /// 身份
    pub identity: Option<String>,
    /// 状态 (alive/dead/retired)
    pub status: String,
    /// 所属服务器 URL
    pub server_url: Option<String>,
    /// 注册时间
    pub registered_at: Option<String>,
    /// 是否为当前活跃角色
    pub is_current: bool,
}

/// 获取所有角色列表
///
/// GET /api/v1/characters
///
/// 返回所有角色（包括已故、归隐的），标记当前活跃角色
pub(super) async fn list_characters_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    // 从配置文件读取完整角色列表
    let config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("读取配置文件失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CharacterListResponse {
                    characters: vec![],
                    current_agent_id: None,
                    current_server_url: state.server_http_url.read().await.clone(),
                }),
            )
                .into_response();
        }
    };

    let current_server_url = state.server_http_url.read().await.clone();
    let current_agent_id = config
        .agent
        .as_ref()
        .and_then(|c| c.agent_id.map(|id| id.to_string()));

    // 从 characters 数组构建列表
    let mut characters: Vec<CharacterInfo> = config
        .characters
        .iter()
        .map(|c| {
            let is_current = c.agent_id.map(|id| id.to_string()) == current_agent_id;
            CharacterInfo {
                agent_id: c.agent_id.map(|id| id.to_string()),
                name: c.name.clone(),
                age: c.age,
                gender: c.gender.clone(),
                identity: c.identity.clone(),
                status: match c.status {
                    crate::config::CharacterStatus::Alive => "alive".to_string(),
                    crate::config::CharacterStatus::Dead => "dead".to_string(),
                    crate::config::CharacterStatus::Retired => "retired".to_string(),
                },
                server_url: c.server_url.clone(),
                registered_at: c.registered_at.map(|t| t.to_rfc3339()),
                is_current,
            }
        })
        .collect();

    // 如果当前 agent 不在 characters 数组中，单独添加
    if let Some(ref current_char) = config.agent {
        let current_char_id = current_char.agent_id.map(|id| id.to_string());
        if !characters.iter().any(|c| c.agent_id == current_char_id) {
            characters.push(CharacterInfo {
                agent_id: current_char_id.clone(),
                name: current_char.name.clone(),
                age: current_char.age,
                gender: current_char.gender.clone(),
                identity: current_char.identity.clone(),
                status: match current_char.status {
                    crate::config::CharacterStatus::Alive => "alive".to_string(),
                    crate::config::CharacterStatus::Dead => "dead".to_string(),
                    crate::config::CharacterStatus::Retired => "retired".to_string(),
                },
                server_url: current_char.server_url.clone(),
                registered_at: current_char.registered_at.map(|t| t.to_rfc3339()),
                is_current: true,
            });
        }
    }

    let current = state.current_state.read().await;
    if let Some(ref ws) = *current {
        let ws_agent_id = ws.agent_id.map(|id| id.to_string());
        for char in characters.iter_mut() {
            if char.agent_id == ws_agent_id {
                if state.is_dead.load(std::sync::atomic::Ordering::Relaxed) {
                    char.status = "dead".to_string();
                } else if let Some(&hp) = ws.self_state.attributes.get("hp") {
                    char.status = if hp > 0 { "alive" } else { "dead" }.to_string();
                }
                break;
            }
        }
    }

    Json(CharacterListResponse {
        characters,
        current_agent_id,
        current_server_url,
    })
    .into_response()
}

/// 切换角色请求
#[derive(Debug, Deserialize)]
pub struct SwitchCharacterRequest {
    /// 目标角色的 agent_id
    pub agent_id: String,
}

/// 切换角色响应
#[derive(Debug, Serialize)]
pub struct SwitchCharacterResponse {
    pub success: bool,
    pub message: String,
    /// 切换后的角色信息
    pub character: Option<CharacterInfo>,
}

/// 切换当前活跃角色
///
/// POST /api/v1/characters/switch
///
/// 切换到指定的角色（必须是已存在的角色）
pub(super) async fn switch_character_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<SwitchCharacterRequest>,
) -> impl IntoResponse {
    // 解析 agent_id
    let agent_id = match Uuid::parse_str(&req.agent_id) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SwitchCharacterResponse {
                    success: false,
                    message: "无效的 agent_id 格式".to_string(),
                    character: None,
                }),
            )
                .into_response();
        }
    };

    // 读取配置文件
    let mut config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("读取配置文件失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SwitchCharacterResponse {
                    success: false,
                    message: format!("读取配置文件失败: {}", e),
                    character: None,
                }),
            )
                .into_response();
        }
    };

    // 查找目标角色
    let target_character = config
        .characters
        .iter()
        .find(|c| c.agent_id == Some(agent_id));

    let character = match target_character {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(SwitchCharacterResponse {
                    success: false,
                    message: "未找到指定的角色".to_string(),
                    character: None,
                }),
            )
                .into_response();
        }
    };

    // 检查角色状态
    if character.status != crate::config::CharacterStatus::Alive {
        return (
            StatusCode::BAD_REQUEST,
            Json(SwitchCharacterResponse {
                success: false,
                message: format!(
                    "无法切换到{}角色",
                    match character.status {
                        crate::config::CharacterStatus::Dead => "已故",
                        crate::config::CharacterStatus::Retired => "归隐",
                        crate::config::CharacterStatus::Alive => "存活",
                    }
                ),
                character: None,
            }),
        )
            .into_response();
    }

    // 执行切换
    if !config.switch_to_character(agent_id) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SwitchCharacterResponse {
                success: false,
                message: "切换角色失败".to_string(),
                character: None,
            }),
        )
            .into_response();
    }

    // 保存配置
    if let Err(e) = config.save_to_file(&state.config_path) {
        error!("保存配置文件失败: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SwitchCharacterResponse {
                success: false,
                message: format!("保存配置文件失败: {}", e),
                character: None,
            }),
        )
            .into_response();
    }

    // 更新内存中的 agent_id
    {
        let mut current_agent_id = state.agent_id.write().await;
        *current_agent_id = agent_id;
    }

    info!("[character] 切换到角色: {} ({})", character.name, agent_id);

    Json(SwitchCharacterResponse {
        success: true,
        message: format!("已切换到角色: {}", character.name),
        character: Some(CharacterInfo {
            agent_id: Some(agent_id.to_string()),
            name: character.name.clone(),
            age: character.age,
            gender: character.gender.clone(),
            identity: character.identity.clone(),
            status: "alive".to_string(),
            server_url: character.server_url.clone(),
            registered_at: character.registered_at.map(|t| t.to_rfc3339()),
            is_current: true,
        }),
    })
    .into_response()
}

// ============================================================================
// 配置管理 API Handlers
// ============================================================================

/// 配置信息响应
#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    /// Server HTTP URL
    pub server_http_url: String,
    /// Server WebSocket URL
    pub server_ws_url: String,
    /// 运行模式
    pub runtime_mode: String,
    /// HTTP 端口
    pub port: u16,
}

// ============================================================================
// 服务器配置 API
// ============================================================================

/// 设置服务器地址请求
#[derive(Debug, Deserialize)]
pub struct SetServerRequest {
    /// WebSocket URL
    pub ws_url: String,
    /// HTTP URL（可选）
    pub http_url: Option<String>,
    /// 确认标记（切换服务器需要二次确认）
    /// 当为 true 时执行切换，否则只返回预览信息和警告
    #[serde(default)]
    pub confirm: bool,
}

/// 设置服务器地址响应
#[derive(Debug, Serialize)]
pub struct SetServerResponse {
    pub success: bool,
    pub message: String,
    pub current: ServerConfigResponse,
    /// 是否需要重新注册设备
    pub needs_device_registration: bool,
    /// 是否需要创建新角色
    pub needs_character_creation: bool,
    /// 该服务器上的历史角色
    pub previous_characters: Vec<CharacterSummary>,
    /// 是否需要二次确认（当 confirm=false 且服务器变化时为 true）
    pub requires_confirmation: bool,
    /// 切换警告信息（当 requires_confirmation=true 时返回）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// 角色摘要信息
#[derive(Debug, Serialize)]
pub struct CharacterSummary {
    pub agent_id: String,
    pub name: String,
    pub status: String,
    pub registered_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ServerConfigResponse {
    pub ws_url: String,
    pub http_url: String,
}

/// 获取当前配置
///
/// GET /api/v1/config - 返回当前运行时配置
pub(super) async fn get_config_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let server_http_url = state.server_http_url.read().await.clone();
    let server_ws_url = state.server_ws_url.read().await.clone();

    Json(ConfigResponse {
        server_http_url,
        server_ws_url,
        runtime_mode: "claw".to_string(),
        port: 23340,
    })
}

/// GET /api/v1/setup/status - 返回引导状态
pub(super) async fn setup_status_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(_) => {
            return Json(dto::SetupStatusResponse {
                needs_setup: true,
                has_server: false,
                has_llm: false,
                has_character: false,
                current_character: None,
            })
            .into_response();
        }
    };

    let has_server = !config.server.ws_url.is_empty();
    let has_llm = config.llm.model.is_some() || config.llm.base_url.is_some();
    let has_character = config.agent.as_ref().is_some_and(|c| c.is_registered());
    let current_character = config
        .agent
        .as_ref()
        .filter(|c| c.is_registered())
        .map(|c| c.name.clone());
    let needs_setup = !has_server || !has_llm;

    Json(dto::SetupStatusResponse {
        needs_setup,
        has_server,
        has_llm,
        has_character,
        current_character,
    })
    .into_response()
}

/// 配置重载请求
#[derive(Debug, Deserialize)]
pub struct ConfigReloadRequest {
    /// 新的 Server HTTP URL（可选）
    pub server_http_url: Option<String>,
    /// 新的 Server WebSocket URL（可选）
    pub server_ws_url: Option<String>,
}

/// 配置重载响应
#[derive(Debug, Serialize)]
pub struct ConfigReloadResponse {
    /// 是否成功
    pub success: bool,
    /// 消息
    pub message: String,
    /// 更新后的配置
    pub config: Option<ConfigResponse>,
}

/// 热重载配置
///
/// POST /api/v1/config/reload - 热重载服务器配置
///
/// 支持两种方式：
/// 1. 不传参数：从配置文件重新加载
/// 2. 传入参数：直接更新配置（不写文件）
pub(super) async fn reload_config_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<ConfigReloadRequest>,
) -> impl IntoResponse {
    use tracing::info;

    // 如果提供了参数，直接更新
    if req.server_http_url.is_some() || req.server_ws_url.is_some() {
        if let Some(http_url) = &req.server_http_url {
            let mut url = state.server_http_url.write().await;
            *url = http_url.clone();
            info!("[config] 已更新 server_http_url: {}", http_url);
        }
        if let Some(ws_url) = &req.server_ws_url {
            let mut url = state.server_ws_url.write().await;
            *url = ws_url.clone();
            info!("[config] 已更新 server_ws_url: {}", ws_url);
        }

        let config = ConfigResponse {
            server_http_url: state.server_http_url.read().await.clone(),
            server_ws_url: state.server_ws_url.read().await.clone(),
            runtime_mode: "claw".to_string(),
            port: 23340,
        };

        return Json(ConfigReloadResponse {
            success: true,
            message: "配置已更新".to_string(),
            config: Some(config),
        })
        .into_response();
    }

    // 没有参数，从配置文件重新加载
    let config_path = state.config_path.clone();
    match crate::config::Config::from_file(&config_path) {
        Ok(config) => {
            // 更新 server URLs
            {
                let mut http_url = state.server_http_url.write().await;
                *http_url = config.server.http_url.clone();
            }
            {
                let mut ws_url = state.server_ws_url.write().await;
                *ws_url = config.server.ws_url.clone();
            }

            info!(
                "[config] 已从文件重载配置: http={}, ws={}",
                config.server.http_url, config.server.ws_url
            );

            let response_config = ConfigResponse {
                server_http_url: config.server.http_url,
                server_ws_url: config.server.ws_url,
                runtime_mode: "claw".to_string(),
                port: 23340,
            };

            Json(ConfigReloadResponse {
                success: true,
                message: "配置已从文件重载".to_string(),
                config: Some(response_config),
            })
            .into_response()
        }
        Err(e) => {
            error!("[config] 重载配置失败: {}", e);
            Json(ConfigReloadResponse {
                success: false,
                message: format!("重载配置失败: {}", e),
                config: None,
            })
            .into_response()
        }
    }
}

/// 保存服务器配置到文件（使用类型化 Config，避免 serde_yaml::Value 破坏其他字段）
///
/// # Arguments
/// * `config_path` - 配置文件完整路径
/// * `ws_url` - WebSocket URL
/// * `http_url` - HTTP URL（可选）
fn save_server_config(
    config_path: &std::path::PathBuf,
    ws_url: &str,
    http_url: Option<&str>,
) -> anyhow::Result<()> {
    let mut config = if config_path.exists() {
        crate::config::Config::from_file(config_path)?
    } else {
        crate::config::Config::default()
    };

    config.server.ws_url = ws_url.to_string();
    if let Some(http) = http_url {
        config.server.http_url = http.to_string();
    }

    config.save_to_file(config_path)?;
    Ok(())
}

/// POST /api/v1/config/server - 设置服务器地址
///
/// 设置服务器地址并触发重连。
/// 配置会持久化到 `config_path` 文件。
///
/// 服务器切换需要二次确认（防止误操作）：
/// 1. 第一次调用（confirm=false）返回预览信息和警告
/// 2. 第二次调用（confirm=true）执行切换
pub(super) async fn set_server_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<SetServerRequest>,
) -> impl IntoResponse {
    // 获取旧的服务器地址
    let old_ws_url = state.server_ws_url.read().await.clone();

    // 计算新的 http_url
    let http_url_value = req.http_url.clone().unwrap_or_else(|| {
        req.ws_url
            .replace("ws://", "http://")
            .replace("wss://", "https://")
            .replace("/ws", "")
    });

    // 检查是否切换到了不同的服务器
    let server_changed = old_ws_url != req.ws_url;

    let mut needs_device_registration = false;
    let mut needs_character_creation = false;
    let mut previous_characters: Vec<CharacterSummary> = vec![];

    // 如果服务器地址变化，检查设备注册状态和角色状态
    if server_changed {
        info!(
            "[config] 检测到服务器地址变更: {} -> {}",
            old_ws_url, req.ws_url
        );

        // 服务器变更后，设备需要重新注册（每个服务器有独立的设备表）
        needs_device_registration = true;

        // 检查是否有该服务器上的存活角色
        let config = crate::config::Config::from_file(&state.config_path).ok();
        if let Some(config) = config {
            // 获取该服务器上的所有角色
            let server_characters = config.get_characters_by_server(&http_url_value);
            previous_characters = server_characters
                .iter()
                .map(|c| CharacterSummary {
                    agent_id: c.agent_id.map(|id| id.to_string()).unwrap_or_default(),
                    name: c.name.clone(),
                    status: match c.status {
                        crate::config::CharacterStatus::Alive => "alive".to_string(),
                        crate::config::CharacterStatus::Dead => "dead".to_string(),
                        crate::config::CharacterStatus::Retired => "retired".to_string(),
                    },
                    registered_at: c.registered_at.map(|t| t.to_rfc3339()),
                })
                .collect();

            // 检查是否有存活角色
            let has_alive = config.has_alive_character_for_server(&http_url_value);
            if !has_alive {
                needs_character_creation = true;
            }
        } else {
            needs_character_creation = true;
        }
    }

    // 如果服务器变化但未确认，返回预览和警告（不执行）
    if server_changed && !req.confirm {
        let warning = format!(
            "切换服务器 {} -> {} 需要二次确认。当前角色状态将被保留，但需要重新连接。",
            old_ws_url, req.ws_url
        );
        return (
            StatusCode::OK,
            Json(SetServerResponse {
                success: true,
                message: "请确认服务器切换".to_string(),
                current: ServerConfigResponse {
                    ws_url: old_ws_url,
                    http_url: state.server_http_url.read().await.clone(),
                },
                needs_device_registration,
                needs_character_creation,
                previous_characters,
                requires_confirmation: true,
                warning: Some(warning),
            }),
        );
    }

    // 1. 更新内存中的配置
    {
        let mut ws_url = state.server_ws_url.write().await;
        *ws_url = req.ws_url.clone();
    }
    {
        let mut url = state.server_http_url.write().await;
        *url = http_url_value.clone();
    }

    // 2. 持久化到配置文件（config_path 是文件路径，非目录）
    if let Err(e) = save_server_config(&state.config_path, &req.ws_url, Some(&http_url_value)) {
        error!("保存配置失败: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SetServerResponse {
                success: false,
                message: format!("保存配置失败: {}", e),
                current: ServerConfigResponse {
                    ws_url: req.ws_url,
                    http_url: http_url_value,
                },
                needs_device_registration: false,
                needs_character_creation: false,
                previous_characters: vec![],
                requires_confirmation: false,
                warning: None,
            }),
        );
    }

    // 3. 触发重连（通过 channel 通知主循环）
    if let Some(ref tx) = state.reconnect_tx {
        let reconnect_req = super::ReconnectRequest {
            ws_url: req.ws_url.clone(),
        };
        if let Err(e) = tx.send(reconnect_req).await {
            error!("发送重连请求失败: {}", e);
        } else {
            info!("[config] 触发 WebSocket 重连: {}", req.ws_url);
        }
    }

    // 构建响应消息
    let message = if needs_device_registration {
        "服务器地址已更新，需要重新注册设备".to_string()
    } else if needs_character_creation {
        "服务器地址已更新，需要创建新角色".to_string()
    } else {
        "服务器地址已更新，正在重连...".to_string()
    };

    let response = SetServerResponse {
        success: true,
        message,
        current: ServerConfigResponse {
            ws_url: req.ws_url,
            http_url: http_url_value,
        },
        needs_device_registration,
        needs_character_creation,
        previous_characters,
        requires_confirmation: false,
        warning: None,
    };

    (StatusCode::OK, Json(response))
}

// ============================================================================
// LLM 配置 API Handlers
// ============================================================================

/// GET /api/v1/config/llm/providers - 返回支持的 LLM Provider 列表
///
/// 注意：此接口不读取 OpenClaw 配置内容，仅检查配置文件是否存在。
/// 遵循"仅当用户选择 openclaw provider 时读取一次"的原则。
/// 如果 OpenClaw 配置文件不存在，则禁选该 Provider。
pub(super) async fn get_llm_providers_handler() -> impl IntoResponse {
    // 仅检查 OpenClaw 配置文件是否存在，不读取内容
    // 遵循"仅当用户选择 openclaw provider 时读取一次"的原则
    let openclaw_config_path = crate::ai::llm::direct_client::OpenClawConfig::config_path();
    let has_openclaw_config = openclaw_config_path
        .as_ref()
        .is_ok_and(|path| path.exists());

    let providers = vec![
        dto::LlmProviderInfo {
            value: "ollama".to_string(),
            label: "Ollama".to_string(),
            requires_base_url: false,
            disabled: None,
            disabled_reason: None,
        },
        dto::LlmProviderInfo {
            value: "openclaw".to_string(),
            label: "OpenClaw Gateway".to_string(),
            requires_base_url: false,
            // 如果配置文件不存在，禁选该 Provider
            disabled: Some(!has_openclaw_config),
            disabled_reason: if !has_openclaw_config {
                Some("OpenClaw 不存在".to_string())
            } else {
                None
            },
        },
        dto::LlmProviderInfo {
            value: "openai_compatible".to_string(),
            label: "OpenAI Compatible".to_string(),
            requires_base_url: true,
            disabled: None,
            disabled_reason: None,
        },
    ];
    Json(dto::LlmProvidersResponse { providers })
}

/// GET /api/v1/config/llm/providers/openclaw/defaults - 返回 OpenClaw 默认配置
///
/// **仅当用户选择 openclaw provider 时调用此接口**
/// 读取 `~/.openclaw/openclaw.json` 获取 gateway_url
/// 注意：不读取 api_key，api_key 必须由用户手动输入
pub(super) async fn get_openclaw_defaults_handler() -> impl IntoResponse {
    use crate::ai::llm::direct_client::OpenClawConfig;

    match OpenClawConfig::load() {
        Ok(config) => {
            let base_url = config.gateway_url().map(|s| s.to_string());
            Json(dto::OpenClawDefaultsResponse {
                base_url,
                model: None, // OpenClaw 配置中没有默认模型
            })
        }
        Err(e) => {
            tracing::warn!("Failed to load OpenClaw config: {}", e);
            Json(dto::OpenClawDefaultsResponse {
                base_url: None,
                model: None,
            })
        }
    }
}

/// GET /api/v1/config/llm - 返回当前 LLM 配置
pub(super) async fn get_llm_config_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("[llm] 读取配置文件失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "config_read_error".to_string(),
                    message: format!("读取配置文件失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    let actor = dto::LlmConfigInfo {
        provider: config.llm.provider.clone(),
        model: config.llm.model.clone().unwrap_or_default(),
        base_url: config.llm.base_url.clone(),
        has_api_key: config.llm.api_key.as_ref().is_some_and(|k| !k.is_empty()),
    };

    let reflector = config.llm_reflector.as_ref().map(|c| dto::LlmConfigInfo {
        provider: c.provider.clone(),
        model: c.model.clone().unwrap_or_default(),
        base_url: c.base_url.clone(),
        has_api_key: c.api_key.as_ref().is_some_and(|k| !k.is_empty()),
    });

    let response = dto::LlmConfigResponse {
        actor,
        reflector,
        reflector_inherits_actor: config.llm_reflector.is_none(),
        runtime_mode: state.runtime_mode.to_string(),
    };

    Json(response).into_response()
}

/// LLM 配置更新响应
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct LlmConfigUpdateResponse {
    pub success: bool,
    pub message: String,
    pub config: Option<dto::LlmConfigResponse>,
}

/// 验证 LLM 配置并创建测试客户端
#[allow(dead_code)]
fn validate_llm_config(
    provider: &str,
    model: &str,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> anyhow::Result<()> {
    // 验证 provider
    if !crate::config::SUPPORTED_PROVIDERS.contains(&provider) {
        anyhow::bail!("不支持的 Provider: {}", provider);
    }

    // 验证 model
    if model.is_empty() {
        anyhow::bail!("model 不能为空");
    }

    // 验证 API Key 格式
    if let Some(key) = api_key {
        crate::config::LlmConfig::validate_api_key(provider, key)?;
    }

    // 检查 requires_base_url 的 provider 是否提供了 base_url
    if provider == "openai_compatible"
        && (base_url.is_none() || base_url.is_none_or(|u| u.is_empty()))
    {
        anyhow::bail!("{} 需要提供 base_url", provider);
    }

    Ok(())
}

/// POST /api/v1/config/llm - 更新 LLM 配置
///
/// 验证配置、测试 LLM 连接、保存配置文件
#[allow(dead_code)]
pub(super) async fn update_llm_config_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<dto::LlmConfigUpdate>,
) -> impl IntoResponse {
    use crate::ai::llm::{DirectLlmClient, DirectLlmClientConfig, LlmClient, LlmProvider};

    // 1. 验证 actor 配置
    if let Err(e) = validate_llm_config(
        &req.actor.provider,
        &req.actor.model,
        req.actor.base_url.as_deref(),
        if req.actor.api_key.is_empty() {
            None
        } else {
            Some(&req.actor.api_key)
        },
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(LlmConfigUpdateResponse {
                success: false,
                message: format!("Actor 配置验证失败: {}", e),
                config: None,
            }),
        )
            .into_response();
    }

    // 2. 验证 reflector 配置（如果有）
    if let Some(ref reflector) = req.reflector
        && let Err(e) = validate_llm_config(
            &reflector.provider,
            &reflector.model,
            reflector.base_url.as_deref(),
            if reflector.api_key.is_empty() {
                None
            } else {
                Some(&reflector.api_key)
            },
        )
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(LlmConfigUpdateResponse {
                success: false,
                message: format!("Reflector 配置验证失败: {}", e),
                config: None,
            }),
        )
            .into_response();
    }

    // 3. 创建测试 LLM 客户端并测试连接
    let provider = match LlmProvider::parse(&req.actor.provider) {
        Some(p) => p,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(LlmConfigUpdateResponse {
                    success: false,
                    message: format!("不支持的 Provider: {}", req.actor.provider),
                    config: None,
                }),
            )
                .into_response();
        }
    };

    let test_config = DirectLlmClientConfig::new(
        provider,
        if req.actor.api_key.is_empty() {
            None::<String>
        } else {
            Some(req.actor.api_key.clone())
        },
    )
    .with_model(&req.actor.model);

    let test_config = if let Some(ref url) = req.actor.base_url {
        test_config.with_base_url(url)
    } else {
        test_config
    };

    let test_client = match DirectLlmClient::new(test_config) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(LlmConfigUpdateResponse {
                    success: false,
                    message: format!("创建 LLM 客户端失败: {}", e),
                    config: None,
                }),
            )
                .into_response();
        }
    };

    // 测试 LLM 连接
    match test_client
        .complete("Hello, this is a connection test. Reply with 'OK'.")
        .await
    {
        Ok(_) => {
            info!(
                "[llm] LLM 连接测试成功: provider={}, model={}",
                req.actor.provider, req.actor.model
            );
        }
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(LlmConfigUpdateResponse {
                    success: false,
                    message: format!("LLM 连接测试失败: {}", e),
                    config: None,
                }),
            )
                .into_response();
        }
    }

    // 4. 读取现有配置
    let mut config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(LlmConfigUpdateResponse {
                    success: false,
                    message: format!("读取配置文件失败: {}", e),
                    config: None,
                }),
            )
                .into_response();
        }
    };

    // 5. 备份原配置
    let backup = config.clone();

    // 6. 更新 LLM 配置
    config.llm = crate::config::LlmConfig {
        provider: req.actor.provider.clone(),
        base_url: req.actor.base_url.clone(),
        api_key: if req.actor.api_key.is_empty() {
            None
        } else {
            Some(req.actor.api_key.clone())
        },
        model: Some(req.actor.model.clone()),
        temperature: config.llm.temperature,
        max_tokens: config.llm.max_tokens,
    };

    // 更新 reflector 配置
    if req.reflector_inherits_actor {
        config.llm_reflector = None;
    } else if let Some(ref reflector) = req.reflector {
        config.llm_reflector = Some(crate::config::LlmConfig {
            provider: reflector.provider.clone(),
            base_url: reflector.base_url.clone(),
            api_key: if reflector.api_key.is_empty() {
                None
            } else {
                Some(reflector.api_key.clone())
            },
            model: Some(reflector.model.clone()),
            temperature: config.llm.temperature,
            max_tokens: config.llm.max_tokens,
        });
    }

    // 7. 保存配置（save_to_file 已内置原子写入）
    if let Err(e) = config.save_to_file(&state.config_path) {
        error!("[llm] 保存配置文件失败: {}", e);
        // 尝试恢复备份
        let _ = backup.save_to_file(&state.config_path);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(LlmConfigUpdateResponse {
                success: false,
                message: format!("保存配置失败: {}", e),
                config: None,
            }),
        )
            .into_response();
    }

    info!(
        "[llm] LLM 配置已更新: provider={}, model={}",
        req.actor.provider, req.actor.model
    );

    // 8. 返回更新后的配置
    let actor = dto::LlmConfigInfo {
        provider: config.llm.provider.clone(),
        model: config.llm.model.clone().unwrap_or_default(),
        base_url: config.llm.base_url.clone(),
        has_api_key: config.llm.api_key.as_ref().is_some_and(|k| !k.is_empty()),
    };

    let reflector = config.llm_reflector.as_ref().map(|c| dto::LlmConfigInfo {
        provider: c.provider.clone(),
        model: c.model.clone().unwrap_or_default(),
        base_url: c.base_url.clone(),
        has_api_key: c.api_key.as_ref().is_some_and(|k| !k.is_empty()),
    });

    let response = dto::LlmConfigResponse {
        actor,
        reflector,
        reflector_inherits_actor: config.llm_reflector.is_none(),
        runtime_mode: state.runtime_mode.to_string(),
    };

    (
        StatusCode::OK,
        Json(LlmConfigUpdateResponse {
            success: true,
            message: "LLM 配置已更新".to_string(),
            config: Some(response),
        }),
    )
        .into_response()
}

/// GET /api/v1/config/llm/usage - 获取 LLM Token 累计使用统计
pub(super) async fn get_llm_usage_handler() -> impl IntoResponse {
    Json(crate::ai::llm::token_usage_tracker().snapshot())
}

// ============================================================================
// 认知上下文端点
// ============================================================================

/// 认知端点返回的人设信息（从 DynamicPersona 提取）
#[derive(Debug, Serialize)]
pub struct CognitivePersonaInfo {
    pub name: String,
    pub personality: Vec<String>,
    pub description: String,
}

/// 简化的世界状态（用于认知上下文）
#[derive(Debug, Serialize)]
pub struct SimplifiedWorldState {
    pub agent_id: Option<String>,
    pub attributes: std::collections::HashMap<String, i32>,
    pub nearby_entities_count: usize,
    pub time: SimplifiedTime,
}

/// 简化的时间
#[derive(Debug, Serialize)]
pub struct SimplifiedTime {
    pub hour: i32,
    pub weather: String,
}

/// 认知上下文响应
#[derive(Debug, Serialize)]
pub struct CognitiveContextResponse {
    pub cognitive_context: CognitiveContext,
    pub persona: Option<CognitivePersonaInfo>,
    pub world_state: SimplifiedWorldState,
}

/// GET /api/v1/cognitive - 获取结构化认知上下文
///
/// 返回引导 OpenClaw LLM 进行四阶段推理的结构化上下文
pub(super) async fn get_cognitive_context_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let current = state.current_state.read().await;

    match current.as_ref() {
        Some(world_state) => {
            let narrative_engine = NarrativeEngine::default();
            let builder = CognitiveContextBuilder::new(narrative_engine, Default::default());

            let (persona_info, persona_ref): (
                Option<CognitivePersonaInfo>,
                Option<crate::ai::persona::dynamic_persona::DynamicPersona>,
            ) = if let Some(ref persona_arc) = state.dynamic_persona {
                persona_arc.read(|p| {
                    let info = CognitivePersonaInfo {
                        name: p.name.clone(),
                        personality: p.traits.keys().take(3).cloned().collect(),
                        description: p.base_description.chars().take(100).collect(),
                    };
                    (Some(info), Some(p.clone()))
                })
            } else {
                (None, None)
            };

            let relationship_store = state.relationship_store.as_deref();
            let cognitive_context =
                builder.build_with_persona(world_state, persona_ref.as_ref(), relationship_store);

            let simplified_world_state = SimplifiedWorldState {
                agent_id: world_state.agent_id.map(|id| id.to_string()),
                attributes: world_state.self_state.attributes.clone(),
                nearby_entities_count: world_state.entities.len(),
                time: SimplifiedTime {
                    hour: world_state.world_time.hour,
                    weather: world_state.world_time.weather.clone(),
                },
            };

            let response = CognitiveContextResponse {
                cognitive_context,
                persona: persona_info,
                world_state: simplified_world_state,
            };

            (StatusCode::OK, Json(response)).into_response()
        }
        None => {
            let error = ErrorResponse {
                error_code: "NO_WORLD_STATE".to_string(),
                message: "No world state available".to_string(),
            };
            (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
        }
    }
}

/// GET /api/v1/events - SSE 实时事件流
///
/// 用于 Web 面板实时接收死亡等事件通知
pub(super) async fn death_events_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let mut death_rx = state.death_event_tx.subscribe();
    let mut tick_rx = state.tick_update_tx.subscribe();

    let stream = async_stream::stream! {
        let data = Bytes::from_static(b"event: connected\ndata: {\"status\":\"connected\"}\n\n");
        yield Ok::<_, std::convert::Infallible>(Frame::data(data));

        loop {
            tokio::select! {
                death_result = tokio::time::timeout(Duration::from_secs(30), death_rx.recv()) => {
                    match death_result {
                        Ok(Ok(msg)) => {
                            if matches!(msg, ServerMessage::AgentDied { .. })
                                && let Ok(json) = serde_json::to_string(&msg) {
                                let data = Bytes::from(format!("event: agent_died\ndata: {}\n\n", json));
                                yield Ok::<_, std::convert::Infallible>(Frame::data(data));
                            }
                        }
                        Ok(Err(_)) => {
                            break;
                        }
                        Err(_) => {
                            let data = Bytes::from(b"event: heartbeat\ndata: {}\n\n".to_vec());
                            yield Ok::<_, std::convert::Infallible>(Frame::data(data));
                        }
                    }
                }
                tick_result = tick_rx.recv() => {
                    match tick_result {
                        Ok(tick_id) => {
                            let json = serde_json::json!({"tick_id": tick_id}).to_string();
                            let data = Bytes::from(format!("event: tick_update\ndata: {}\n\n", json));
                            yield Ok::<_, std::convert::Infallible>(Frame::data(data));
                        }
                        Err(_) => {
                            break;
                        }
                    }
                }
            }
        }
    };

    let body = StreamBody::new(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .header("X-Accel-Buffering", "no")
        .body(body)
        .unwrap()
}
