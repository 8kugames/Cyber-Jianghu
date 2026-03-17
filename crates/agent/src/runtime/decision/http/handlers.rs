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

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tracing::error;

use cyber_jianghu_protocol::{Intent, ActionType};
use crate::ai::lifespan::LifespanStatus;
use crate::ai::validator::{ValidationRequest, ValidationResult, PersonaInfo};

use super::{HttpApiState, IntentRequest};
use super::context::{ContextResponse, generate_context_markdown, generate_context_markdown_no_relationship, create_attributes_glimpse};
use super::dto::{
    HealthResponse, RelationshipUpdateRequest, LifespanResponse,
    ValidateRequest, ValidateResponse,
};
use super::service::{RelationshipService, MemoryService, memories_to_json_response};

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
    ];

    Json(ApiListResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        agent_id: state.agent_id.to_string(),
        endpoints,
    })
}

// ============================================================================
// 基础端点 Handlers
// ============================================================================

/// Health check handler
pub(super) async fn health_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let current = state.current_state.read().await;
    let response = HealthResponse {
        status: "ok".to_string(),
        agent_id: state.agent_id.to_string(),
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
    match current.as_ref() {
        Some(world_state) => {
            // 使用叙事引擎生成上下文，不暴露原始数值
            let engine = state.narrative_engine.as_ref()
                .map(|e| e.as_ref())
                .unwrap_or_else(|| {
                    // 如果没有初始化，使用内置配置创建临时引擎
                    static DEFAULT_ENGINE: std::sync::OnceLock<crate::ai::cognitive::narrative::NarrativeEngine> = std::sync::OnceLock::new();
                    DEFAULT_ENGINE.get_or_init(crate::ai::cognitive::narrative::NarrativeEngine::with_builtin_config)
                });

            let context = if let Some(store) = &state.relationship_store {
                generate_context_markdown(world_state, store, engine)
            } else {
                generate_context_markdown_no_relationship(world_state, engine)
            };
            Json(ContextResponse {
                context,
                tick_id: world_state.tick_id,
                agent_id: state.agent_id.to_string(),
            }).into_response()
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
            let engine = state.narrative_engine.as_ref()
                .map(|e| e.as_ref())
                .unwrap_or_else(|| {
                    static DEFAULT_ENGINE: std::sync::OnceLock<crate::ai::cognitive::narrative::NarrativeEngine> = std::sync::OnceLock::new();
                    DEFAULT_ENGINE.get_or_init(crate::ai::cognitive::narrative::NarrativeEngine::with_builtin_config)
                });

            let glimpse = create_attributes_glimpse(world_state, engine);
            Json(glimpse).into_response()
        }
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Submit intent handler (完全数据驱动)
pub(super) async fn submit_intent_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<IntentRequest>,
) -> impl IntoResponse {
    let agent_id = req.agent_id.as_ref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or(state.agent_id);

    let tick_id = req.tick_id.unwrap_or(0);
    let action_type: ActionType = req.action_type.into();
    let intent = Intent::new(agent_id, tick_id, action_type, req.action_data);

    match state.intent_tx.send(intent).await {
        Ok(_) => (StatusCode::OK, "Intent submitted").into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Failed to submit intent").into_response(),
    }
}

// ============================================================================
// 关系 API Handlers
// ============================================================================

/// 获取所有关系
pub(super) async fn get_relationships_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let store = match &state.relationship_store {
        Some(s) => s,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "Relationship store not initialized").into_response(),
    };

    let service = RelationshipService::new(store);
    match service.get_all() {
        Ok(relationships) => Json(relationships).into_response(),
        Err(e) => {
            error!("[http] Failed to get relationships: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to get relationships: {}", e)).into_response()
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
        None => return (StatusCode::SERVICE_UNAVAILABLE, "Relationship store not initialized").into_response(),
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
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to get relationship: {}", e)).into_response()
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
        None => return (StatusCode::SERVICE_UNAVAILABLE, "Relationship store not initialized").into_response(),
    };

    let target_id = match Uuid::parse_str(&req.target_agent_id) {
        Ok(uuid) => uuid,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid target_agent_id format").into_response(),
    };

    let tick_id = state.current_state.read().await.as_ref().map(|s| s.tick_id).unwrap_or(0);

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
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to update relationship: {}", e)).into_response()
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
        None => return (StatusCode::SERVICE_UNAVAILABLE, "Lifespan calculator not initialized").into_response(),
    };

    let calc = calculator.lock().await;
    let response = match calc.get_status() {
        LifespanStatus::Alive { age } => LifespanResponse {
            current_age: age, status: "alive".to_string(), aging_effects: None,
        },
        LifespanStatus::Aging { age, effects } => LifespanResponse {
            current_age: age, status: "aging".to_string(), aging_effects: Some(format!("{:?}", effects)),
        },
        LifespanStatus::Deceased { age } => LifespanResponse {
            current_age: age, status: "deceased".to_string(), aging_effects: None,
        },
    };
    drop(calc);

    Json(response).into_response()
}

// ============================================================================
// 记忆 API Handlers
// ============================================================================

/// 获取近期记忆
pub(super) async fn get_recent_memory_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
    let manager = match &state.memory_manager {
        Some(m) => m,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "Memory manager not initialized").into_response(),
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
        None => return (StatusCode::SERVICE_UNAVAILABLE, "Memory manager not initialized").into_response(),
    };

    let mut mgr = manager.lock().await;
    let mut service = MemoryService::new(&mut mgr);
    let limit = request.limit.unwrap_or(10);

    match service.search(&request.query, limit).await {
        Ok(memories) => Json(memories_to_json_response(&memories)).into_response(),
        Err(e) => {
            error!("[http] Failed to search memory: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Search failed: {}", e)).into_response()
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
        None => return (StatusCode::SERVICE_UNAVAILABLE, "Memory manager not initialized").into_response(),
    };

    let tick_id = state.current_state.read().await.as_ref().map(|s| s.tick_id).unwrap_or(0);
    let mut mgr = manager.lock().await;
    let mut service = MemoryService::new(&mut mgr);

    match service.store(state.agent_id, tick_id, req.content, req.importance).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"success": true, "message": "Memory stored"}))).into_response(),
        Err(e) => {
            error!("[http] Failed to store memory: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to store memory: {}", e)).into_response()
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
        }).into_response();
    }

    let validator = match &state.intent_validator {
        Some(v) => v,
        None => return Json(ValidateResponse {
            valid: true, reason: None, rejection_type: None, narrative: None,
        }).into_response(),
    };

    let agent_id = req.agent_id.as_ref()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or(state.agent_id);

    let intent = Intent::new(agent_id, req.tick_id.unwrap_or(0), req.action_type, req.action_data);

    let persona_info = PersonaInfo {
        gender: req.persona_gender.unwrap_or_else(|| "未知".to_string()),
        age: req.persona_age.unwrap_or(28),
        personality: req.persona_personality.unwrap_or_default(),
        values: req.persona_values.unwrap_or_default(),
    };

    let world_state = state.current_state.read().await;
    let world_context = world_state.as_ref()
        .map(|ws| format!("Tick: {}, Location: {:?}", ws.tick_id, ws.location))
        .unwrap_or_else(|| "No world state available".to_string());
    drop(world_state);

    let validation_req = ValidationRequest { intent, persona: persona_info, world_context };

    match validator.validate(validation_req).await {
        Ok(ValidationResult::Approved { reason, narrative }) => Json(ValidateResponse {
            valid: true, reason, rejection_type: None, narrative: Some(narrative),
        }).into_response(),
        Ok(ValidationResult::Rejected { reason, rejection_type }) => Json(ValidateResponse {
            valid: false, reason: Some(reason), rejection_type: Some(rejection_type.as_str().to_string()), narrative: None,
        }).into_response(),
        Err(e) => {
            error!("[http] Validation error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Validation error: {}", e)).into_response()
        }
    }
}
