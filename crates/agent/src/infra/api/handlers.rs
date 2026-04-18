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

use anyhow::Context;
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
use std::path::Path;
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::{CharacterConfig, CharacterStatus};

use crate::component::persona::LifespanStatus;
use crate::soul::reflector::{PersonaInfo, ValidationRequest, ValidationResult};
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
// Helper Functions for Character Management
// ============================================================================

/// List all characters from filesystem
fn list_characters_from_fs(characters_dir: &Path) -> Result<Vec<CharacterConfig>, anyhow::Error> {
    if !characters_dir.exists() {
        return Ok(vec![]);
    }
    let mut chars = vec![];
    for entry in std::fs::read_dir(characters_dir).context("Failed to read characters dir")? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let char_yaml = entry.path().join("character.yaml");
        if !char_yaml.exists() {
            continue;
        }
        match CharacterConfig::from_file(&char_yaml) {
            Ok(c) => chars.push(c),
            Err(e) => warn!(
                "Skipping corrupted character.yaml in {:?}: {}",
                entry.path(),
                e
            ),
        }
    }
    Ok(chars)
}

/// Get active (alive) character from state
async fn get_active_character(
    state: &HttpApiState,
) -> Result<Option<CharacterConfig>, anyhow::Error> {
    let character_dir = state.character_dir.read().await.clone();
    let chars = list_characters_from_fs(&character_dir)?;
    Ok(chars
        .into_iter()
        .find(|c| c.status == CharacterStatus::Alive))
}

/// Get character config by agent_id (sync version for use in handlers)
fn get_character_by_id_sync(
    characters_dir: &std::path::Path,
    agent_id: Uuid,
) -> Result<Option<CharacterConfig>, anyhow::Error> {
    let chars = list_characters_from_fs(characters_dir)?;
    Ok(chars.into_iter().find(|c| c.agent_id == Some(agent_id)))
}

/// Save character config to its directory
fn save_character(config: &CharacterConfig, characters_dir: &Path) -> Result<(), anyhow::Error> {
    let agent_id = config
        .agent_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let dir = characters_dir.join(&agent_id);
    std::fs::create_dir_all(&dir)?;
    config.save_to_file(dir.join("character.yaml"))
}

/// Get device identity from state (async-safe)
async fn get_device_id(state: &HttpApiState) -> Result<(Uuid, String), anyhow::Error> {
    let device = state.device_config.read().await;
    let d = device.as_ref().context("No device identity")?;
    Ok((d.device_id, d.auth_token.clone()))
}

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
            description: "结构化认知上下文（引导 OpenClaw 按阶段推理）".to_string(),
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
        // === 性能指标 ===
        ApiEndpoint {
            path: "/api/v1/metrics".to_string(),
            method: "GET".to_string(),
            description: "LLM 性能指标（调用次数、失败率、token 使用）".to_string(),
            request_example: None,
            response_example: Some(serde_json::json!({
                "llm": [{
                    "provider": "minimax",
                    "model": "M2.7-highspeed",
                    "calls": 1234,
                    "failures": 5,
                    "success_rate": "99%",
                    "prompt_tokens": 100000,
                    "completion_tokens": 50000,
                    "total_tokens": 150000
                }]
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
            let context = if let Some(store) = &state.relationship_store {
                generate_context_markdown(world_state, store, dream_thought.as_deref())
            } else {
                generate_context_markdown_no_relationship(world_state, dream_thought.as_deref())
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
            let glimpse = create_attributes_glimpse(world_state);
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
        world_state: None,
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
    use crate::component::llm::{
        DirectLlmClient, DirectLlmClientConfig, LlmClientExt, LlmProvider,
    };

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
时代：武侠架空世界，冷兵器时代。世界使用独立"天道历"纪年，与现实朝代无关。
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
    let (device_id, auth_token) = match get_device_id(&state).await {
        Ok(id) => id,
        Err(e) => {
            error!("设备身份未初始化: {}", e);
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
            "你是{}，{}岁，{}。{}{}你的目标是探索这个江湖世界，与各路侠客交流，并在武林中闯出自己的一片天地。",
            payload.name,
            payload.age,
            payload.identity.as_deref().unwrap_or("江湖中人"),
            payload.appearance.as_deref().map(|a| a.to_string()).unwrap_or_default(),
            if !payload.personality.is_empty() {
                format!("性格特点：{}。", payload.personality.join("、"))
            } else {
                String::new()
            }
        )
    });

    // 3. 构建发送到 Server 的请求
    let server_request = serde_json::json!({
        "device_id": device_id,
        "auth_token": auth_token,
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

    let mut response = match client.post(&server_url).json(&server_request).send().await {
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

        if status == StatusCode::UNAUTHORIZED {
            warn!("收到 401，尝试刷新令牌后重试...");
            if let Err(e) = state.refresh_auth_token().await {
                error!("刷新令牌失败: {}", e);
                let _body = response.text().await.unwrap_or_default();
                return (
                    status,
                    Json(CharacterRegisterResponse {
                        agent_id: String::new(),
                        message: format!("认证失败且刷新令牌失败: {}", e),
                        warning: None,
                    }),
                )
                    .into_response();
            }

            let (device_id, auth_token) = match get_device_id(&state).await {
                Ok(id) => id,
                Err(e) => {
                    error!("刷新令牌后获取设备ID失败: {}", e);
                    let _body = response.text().await.unwrap_or_default();
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(CharacterRegisterResponse {
                            agent_id: String::new(),
                            message: format!("刷新令牌后获取设备ID失败: {}", e),
                            warning: None,
                        }),
                    )
                        .into_response();
                }
            };

            let server_request = serde_json::json!({
                "device_id": device_id,
                "auth_token": auth_token,
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

            let retry_response = match client.post(&server_url).json(&server_request).send().await {
                Ok(resp) => resp,
                Err(e) => {
                    error!("重试连接服务器失败: {}", e);
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

            if !retry_response.status().is_success() {
                let status = retry_response.status();
                let body = retry_response.text().await.unwrap_or_default();
                error!("重试后服务器拒绝注册: {} - {}", status, body);
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

            response = retry_response;
        } else {
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
    }

    // 6. 解析成功响应
    #[derive(Deserialize)]
    struct ServerRegisterResponse {
        agent_id: String,
        message: String,
        #[allow(dead_code)]
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

            // 8. 创建并保存角色配置到文件系统
            let mut config_warning = None;
            let agent_uuid = match uuid::Uuid::parse_str(&result.agent_id) {
                Ok(id) => id,
                Err(e) => {
                    error!("解析 agent_id 失败: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(CharacterRegisterResponse {
                            agent_id: String::new(),
                            message: format!("解析 agent_id 失败: {}", e),
                            warning: None,
                        }),
                    )
                        .into_response();
                }
            };

            let new_character = CharacterConfig {
                agent_id: Some(agent_uuid),
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
                registered_at: Some(chrono::Utc::now()),
                birth_attributes: if result.initial_attributes.is_empty() {
                    None
                } else {
                    Some(result.initial_attributes.clone())
                },
                status: CharacterStatus::Alive,
                server_url: Some(server_http_url.clone()),
                last_connected_real_time: None,
                last_connected_world_time: None,
            };

            if let Err(e) = save_character(&new_character, &state.character_dir.read().await) {
                error!("保存角色配置失败: {}", e);
                config_warning = Some(format!("角色配置保存失败: {}", e));
            }

            // 9. 更新运行时 agent_id（使后续 Intent 提交使用新角色）
            {
                let mut id = state.agent_id.write().await;
                *id = agent_uuid;
                info!(
                    "[character] Updated runtime agent_id to {} ({})",
                    agent_uuid, payload.name
                );
            }

            // 10. 重置死亡状态（新角色 = 新生命）
            state
                .is_dead
                .store(false, std::sync::atomic::Ordering::Relaxed);

            // 11. 触发 WebSocket 重连以注册新角色
            if let Some(ref tx) = state.reconnect_tx {
                let server_ws_url = state.server_ws_url.read().await.clone();
                let reconnect_req = super::ReconnectRequest {
                    ws_url: server_ws_url,
                };
                if let Err(e) = tx.send(reconnect_req) {
                    error!("[character] 注册后触发重连失败: {}", e);
                } else {
                    info!("[character] 注册后触发 WebSocket 重连");
                }
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
    /// 服务器地址
    pub server_url: Option<String>,
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
    /// 先天属性（注册时的属性值）
    pub birth_attributes: Option<serde_json::Value>,
    /// 持有物品
    pub inventory: Option<serde_json::Value>,
    /// 派生属性（带叙事描述）
    pub derived_attributes: Option<serde_json::Value>,
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
    // 1. 从文件系统读取活跃角色配置
    let character = match get_active_character(&state).await {
        Ok(Some(ch)) => ch,
        Ok(None) => {
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(ErrorResponse {
                    error_code: "character_not_registered".to_string(),
                    message: "角色尚未注册，请先创建角色".to_string(),
                }),
            )
                .into_response();
        }
        Err(e) => {
            error!("读取角色配置失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "character_read_error".to_string(),
                    message: format!("读取角色配置失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    // 2. 加载叙事配置（用于属性描述）
    let narrative_config = state.narrative_config.read().await.clone();

    // 3. 从当前 WorldState 获取实时状态
    let current = state.current_state.read().await;

    // 是否使用缓存数据（当角色已死或服务器未连接时）
    let is_dead_flag = state.is_dead.load(std::sync::atomic::Ordering::Relaxed);
    let is_stale = current.is_none() || is_dead_flag;

    let (agent_id, raw_attributes, inventory, location, tick_id, world_time) =
        match current.as_ref() {
            Some(ws) => {
                let agent_id = ws.agent_id.map(|id| id.to_string());
                let attrs = serde_json::to_value(&ws.self_state.attributes).ok();
                let inv = serde_json::to_value(&ws.self_state.inventory).ok();
                let loc = Some(format!("{} ({})", ws.location.name, ws.location.node_type));
                let time = enrich_world_time_json(&ws.world_time);
                (agent_id, attrs, inv, loc, Some(ws.tick_id), time)
            }
            None => {
                // 降级使用配置数据（birth_attributes 作为 attributes 的兜底）
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
                )
            }
        };

    // 4. 计算角色状态（在 move attributes 之前）
    // 优先使用 is_dead 标志（当 AgentDied 消息已收到但 WorldState 尚未更新时）
    let status = if state.is_dead.load(std::sync::atomic::Ordering::Relaxed) {
        Some("dead".to_string())
    } else {
        raw_attributes
            .as_ref()
            .and_then(|a| a.get("hp"))
            .and_then(|hp| hp.as_i64())
            .map(|hp| if hp > 0 { "alive" } else { "dead" }.to_string())
            .or_else(|| match character.status {
                CharacterStatus::Dead => Some("dead".to_string()),
                CharacterStatus::Retired => Some("retired".to_string()),
                CharacterStatus::Alive => Some("alive".to_string()),
            })
    };

    // 5. 丰富属性数据（添加叙事描述）
    let attributes = enrich_attributes_with_descriptions(raw_attributes, &narrative_config);

    // 6. 获取服务器地址
    let current_server_url = state.server_http_url.read().await.clone();
    let server_url = character.server_url.clone().or(Some(current_server_url));

    // 7. 构建响应
    let response = CharacterInfoResponse {
        agent_id,
        server_url,
        name: character.name.clone(),
        age: character.age,
        gender: character.gender.clone(),
        appearance: character.appearance.clone(),
        identity: character.identity.clone(),
        personality: character.personality.clone(),
        values: character.values.clone(),
        registered_at: character.registered_at.map(|t| t.to_rfc3339()),
        attributes,
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
        derived_attributes: enrich_derived_attributes(
            current
                .as_ref()
                .map(|ws| ws.self_state.derived_attributes.clone()),
            &narrative_config,
        ),
    };

    Json(response).into_response()
}

/// GET /api/v1/characters/:id
///
/// 获取指定角色的完整信息（用于抽屉展示）
pub(super) async fn get_character_by_id_handler(
    State(state): State<HttpApiState>,
    AxumPath(agent_id): AxumPath<Uuid>,
) -> impl IntoResponse {
    // 1. 从文件系统读取角色配置
    let character_dir = state.character_dir.read().await.clone();
    let character = match get_character_by_id_sync(&character_dir, agent_id) {
        Ok(Some(ch)) => ch,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error_code: "character_not_found".to_string(),
                    message: "角色不存在".to_string(),
                }),
            )
                .into_response();
        }
        Err(e) => {
            error!("读取角色配置失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error_code: "character_read_error".to_string(),
                    message: format!("读取角色配置失败: {}", e),
                }),
            )
                .into_response();
        }
    };

    // 2. 如果是当前角色，返回完整 WorldState 数据
    let current_agent_id = *state.agent_id.read().await;
    let is_current = current_agent_id == agent_id;

    if is_current {
        // 复用当前角色的 WorldState 数据
        let narrative_config = state.narrative_config.read().await.clone();
        let current = state.current_state.read().await;
        let is_dead_flag = state.is_dead.load(std::sync::atomic::Ordering::Relaxed);
        let is_stale = current.is_none() || is_dead_flag;

        let (raw_attributes, inventory, location, tick_id, world_time) = match current.as_ref() {
            Some(ws) => {
                let attrs = serde_json::to_value(&ws.self_state.attributes).ok();
                let inv = serde_json::to_value(&ws.self_state.inventory).ok();
                let loc = Some(format!("{} ({})", ws.location.name, ws.location.node_type));
                let time = enrich_world_time_json(&ws.world_time);
                (attrs, inv, loc, Some(ws.tick_id), time)
            }
            None => (None, None, None, None, None),
        };

        let status = if state.is_dead.load(std::sync::atomic::Ordering::Relaxed) {
            Some("dead".to_string())
        } else {
            raw_attributes
                .as_ref()
                .and_then(|a| a.get("hp"))
                .and_then(|hp| hp.as_i64())
                .map(|hp| if hp > 0 { "alive" } else { "dead" }.to_string())
        };

        let attributes = enrich_attributes_with_descriptions(raw_attributes, &narrative_config);
        let current_server_url = state.server_http_url.read().await.clone();
        let server_url = character.server_url.clone().or(Some(current_server_url));

        return Json(CharacterInfoResponse {
            agent_id: character.agent_id.map(|id| id.to_string()),
            server_url,
            name: character.name.clone(),
            age: character.age,
            gender: character.gender.clone(),
            appearance: character.appearance.clone(),
            identity: character.identity.clone(),
            personality: character.personality.clone(),
            values: character.values.clone(),
            registered_at: character.registered_at.map(|t| t.to_rfc3339()),
            attributes,
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
            derived_attributes: enrich_derived_attributes(
                current
                    .as_ref()
                    .map(|ws| ws.self_state.derived_attributes.clone()),
                &narrative_config,
            ),
        })
        .into_response();
    }

    // 3. 非当前角色，返回配置文件数据（不包含实时状态）
    let current_server_url = state.server_http_url.read().await.clone();
    let server_url = character.server_url.clone().or(Some(current_server_url));

    // 非当前角色也做属性丰富化，以便前端正确渲染
    let raw_attrs = character
        .birth_attributes
        .as_ref()
        .and_then(|a| serde_json::to_value(a).ok());
    let narrative_config = state.narrative_config.read().await.clone();
    let attributes = enrich_attributes_with_descriptions(raw_attrs.clone(), &narrative_config);

    let response = CharacterInfoResponse {
        agent_id: character.agent_id.map(|id| id.to_string()),
        server_url,
        name: character.name.clone(),
        age: character.age,
        gender: character.gender.clone(),
        appearance: character.appearance.clone(),
        identity: character.identity.clone(),
        personality: character.personality.clone(),
        values: character.values.clone(),
        registered_at: character.registered_at.map(|t| t.to_rfc3339()),
        attributes,
        birth_attributes: raw_attrs,
        inventory: None,
        location: None,
        tick_id: None,
        world_time: None,
        status: Some(match character.status {
            CharacterStatus::Alive => "alive".to_string(),
            CharacterStatus::Dead => "dead".to_string(),
            CharacterStatus::Retired => "retired".to_string(),
        }),
        is_stale: true,
        derived_attributes: None,
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
/// 为 WorldTime JSON 添加 `display` 字段（中文格式）
fn enrich_world_time_json(
    world_time: &cyber_jianghu_protocol::WorldTime,
) -> Option<serde_json::Value> {
    let mut val = serde_json::to_value(world_time).ok()?;
    if let Some(obj) = val.as_object_mut() {
        obj.insert(
            "display".to_string(),
            serde_json::Value::String(world_time.to_chinese()),
        );
    }
    Some(val)
}

///
/// 从服务器返回的原始属性中：
/// - 提取 `{key}_max` 字段作为属性最大值（服务器通过 max_value_formula 计算）
/// - 如果没有 `{key}_max` 字段，说明该属性没有上限（如声望、派生属性）
fn enrich_attributes_with_descriptions(
    raw_attributes: Option<serde_json::Value>,
    narrative_config: &Option<cyber_jianghu_protocol::NarrativeConfig>,
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
    derived: Option<std::collections::HashMap<String, f32>>,
    narrative_config: &Option<cyber_jianghu_protocol::NarrativeConfig>,
) -> Option<serde_json::Value> {
    let derived = derived?;
    let enriched: serde_json::Map<String, serde_json::Value> = derived
        .into_iter()
        .map(|(key, value)| {
            let (display_name, description) = narrative_config
                .as_ref()
                .and_then(|cfg| cfg.attributes.get(&key))
                .map(|attr_cfg| {
                    let name = attr_cfg.display_name.clone();
                    let desc = attr_cfg
                        .thresholds
                        .iter()
                        .rev()
                        .find(|t| (value as i32) >= t.min && (value as i32) <= t.max)
                        .map(|t| t.description.clone())
                        .unwrap_or_else(|| format!("{}: {:.3}", name, value));
                    (name, desc)
                })
                .unwrap_or_else(|| (key.clone(), format!("{}: {:.3}", key, value)));

            let attr_obj = serde_json::json!({
                "name": display_name,
                "current": value,
                "description": description,
            });
            (key, attr_obj)
        })
        .collect();

    if enriched.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(enriched))
    }
}

// ============================================================================
// 三魂循环记录 API
// ============================================================================

/// Layer 结果条目
#[derive(Debug, Serialize)]
struct LayerResultEntry {
    layer: String,
    passed: bool,
    detail: Option<String>,
}

/// 人魂记录
#[derive(Debug, Serialize)]
struct RenhunEntry {
    narrative: Option<String>,
    thought_log: Option<String>,
}

/// 天魂审查记录
#[derive(Debug, Serialize)]
struct TianhunEntry {
    result: Option<String>,
    layers: Vec<LayerResultEntry>,
    reason: Option<String>,
    narrative: Option<String>,
}

/// 最终 Intent 记录
#[derive(Debug, Serialize)]
struct FinalIntentEntry {
    intent_id: Option<String>,
    action_type: Option<String>,
    action_data: Option<serde_json::Value>,
}

/// 单条三魂尝试记录
#[derive(Debug, Serialize)]
struct SoulCycleAttemptEntry {
    tick_id: i64,
    world_time: Option<serde_json::Value>,
    created_at: String,
    attempt: i32,
    renhun: RenhunEntry,
    tianhun: TianhunEntry,
    final_intent: Option<FinalIntentEntry>,
}

/// 即时意图记录
#[derive(Debug, Serialize)]
struct ImmediateIntentEntry {
    intent_id: String,
    route_type: String,
    action_type: String,
    action_data: Option<serde_json::Value>,
    speech_content: Option<String>,
    send_status: String,
    send_error: Option<String>,
}

/// 三魂循环完整记录响应
#[derive(Debug, Serialize)]
struct SoulCyclesResponse {
    tick_id: i64,
    attempts: Vec<SoulCycleAttemptEntry>,
    immediate_intents: Vec<ImmediateIntentEntry>,
}

/// 三魂循环分页响应（按 tick 分组）
#[derive(Debug, Serialize)]
struct SoulCyclesPageResponse {
    page: u32,
    limit: u32,
    total: u32,
    has_more: bool,
    records: std::collections::HashMap<String, Vec<SoulCycleAttemptEntry>>,
    immediate_intents: std::collections::HashMap<String, Vec<ImmediateIntentEntry>>,
}

/// SoulCycleRecord → SoulCycleAttemptEntry 转换（消除重复代码）
fn record_to_attempt_entry(
    r: super::soul_cycle_recorder::SoulCycleRecord,
) -> SoulCycleAttemptEntry {
    let action_data: Option<serde_json::Value> = r
        .final_action_data
        .as_ref()
        .and_then(|s| serde_json::from_str(s).ok());
    let layers = [
        (r.tianhun_layer1_result.as_deref(), "layer1"),
        (r.tianhun_layer2_result.as_deref(), "layer2"),
        (r.tianhun_layer3_result.as_deref(), "layer3"),
    ]
    .iter()
    .map(|(detail, layer)| {
        let passed = detail.map(|d| d == "通过" || d.is_empty()).unwrap_or(true);
        LayerResultEntry {
            layer: layer.to_string(),
            passed,
            detail: if passed {
                None
            } else {
                Some(detail.unwrap_or("驳回").to_string())
            },
        }
    })
    .collect();
    let world_time: Option<serde_json::Value> = r.world_time.as_ref().and_then(|s| {
        serde_json::from_str(s)
            .ok()
            .or_else(|| Some(serde_json::Value::String(s.clone())))
    });

    SoulCycleAttemptEntry {
        tick_id: r.tick_id,
        world_time,
        created_at: r.created_at.to_rfc3339(),
        attempt: r.attempt,
        renhun: RenhunEntry {
            narrative: r.renhun_narrative,
            thought_log: r.renhun_thought_log,
        },
        tianhun: TianhunEntry {
            result: r.tianhun_result,
            layers,
            reason: r.tianhun_reason,
            narrative: r.previous_round_narrative,
        },
        final_intent: r.final_intent_id.map(|id| FinalIntentEntry {
            intent_id: Some(id),
            action_type: r.final_action_type,
            action_data,
        }),
    }
}

/// ImmediateIntentRecord → ImmediateIntentEntry 转换
fn immediate_record_to_entry(
    r: super::soul_cycle_recorder::ImmediateIntentRecord,
) -> ImmediateIntentEntry {
    let action_data: Option<serde_json::Value> = r
        .action_data
        .as_ref()
        .and_then(|s| serde_json::from_str(s).ok());
    ImmediateIntentEntry {
        intent_id: r.intent_id,
        route_type: r.route_type,
        action_type: r.action_type,
        action_data,
        speech_content: r.speech_content,
        send_status: r.send_status,
        send_error: r.send_error,
    }
}

/// 获取指定角色的三魂完整记录
///
/// GET /api/v1/character/soul-cycles?tick_id=123
/// GET /api/v1/character/soul-cycles?page=1&limit=20
/// GET /api/v1/character/soul-cycles?agent_id=xxx&page=1&limit=20  # 指定角色
pub(super) async fn get_soul_cycles_handler(
    State(state): State<HttpApiState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let tick_id: Option<i64> = params.get("tick_id").and_then(|s| s.parse().ok());
    let page: u32 = params.get("page").and_then(|s| s.parse().ok()).unwrap_or(1);
    let limit: u32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20)
        .min(50);

    // 确定查询目标角色：优先使用 agent_id 参数，否则用当前角色
    let target_agent_id = if let Some(id_str) = params.get("agent_id") {
        match uuid::Uuid::parse_str(id_str) {
            Ok(id) => id,
            Err(_) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "Invalid agent_id format"})),
                )
                    .into_response();
            }
        }
    } else {
        *state.agent_id.read().await
    };

    let Some(recorder) = state.soul_recorder_for(target_agent_id).await else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Soul cycle record not found for this agent"})),
        )
            .into_response();
    };

    if let Some(tid) = tick_id {
        // 按 tick_id 查询
        let records = recorder.get_by_tick(tid).await;
        let immediate = recorder.get_immediate_by_tick(tid).await;

        let attempts: Vec<SoulCycleAttemptEntry> =
            records.into_iter().map(record_to_attempt_entry).collect();

        let immediate_intents: Vec<ImmediateIntentEntry> = immediate
            .into_iter()
            .map(immediate_record_to_entry)
            .collect();

        Json(SoulCyclesResponse {
            tick_id: tid,
            attempts,
            immediate_intents,
        })
        .into_response()
    } else {
        // 分页查询：按 tick_id 分组
        let (tick_ids, total) = recorder.get_tick_ids_page(page, limit).await;

        // 批量获取所有 tick 的记录和即时意图
        let all_records = recorder.get_by_ticks(&tick_ids).await;
        let all_immediate = recorder.get_immediate_by_ticks(&tick_ids).await;

        // 按 tick_id 分组记录
        let mut records_map: std::collections::HashMap<String, Vec<SoulCycleAttemptEntry>> =
            std::collections::HashMap::new();
        for r in all_records {
            let tick_key = r.tick_id.to_string();
            let entry = record_to_attempt_entry(r);
            records_map.entry(tick_key).or_default().push(entry);
        }

        // 按 tick_id 分组即时意图
        let mut immediate_map: std::collections::HashMap<String, Vec<ImmediateIntentEntry>> =
            std::collections::HashMap::new();
        for imm in all_immediate {
            let tick_key = imm.tick_id.to_string();
            let entry = immediate_record_to_entry(imm);
            immediate_map.entry(tick_key).or_default().push(entry);
        }

        let has_more = (page * limit) < total;
        Json(SoulCyclesPageResponse {
            page,
            limit,
            total,
            has_more,
            records: records_map,
            immediate_intents: immediate_map,
        })
        .into_response()
    }
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
    let (device_id, auth_token) = match get_device_id(&state).await {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::PRECONDITION_FAILED,
                Json(RebirthResponse {
                    success: false,
                    message: format!("设备身份未初始化: {}", e),
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
        device_id,
        auth_token,
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

    // 7. 更新文件系统中的角色配置：标记为 Retired
    if !is_dead_character {
        // 正常归隐：读取角色配置，标记为 Retired
        let character_dir = state.character_dir.read().await.clone();
        let char_dir = character_dir.join(agent_id.to_string());
        let char_yaml = char_dir.join("character.yaml");
        if char_yaml.exists() {
            match CharacterConfig::from_file(&char_yaml) {
                Ok(mut char_config) => {
                    char_config.status = CharacterStatus::Retired;
                    if let Err(e) = char_config.save_to_file(&char_yaml) {
                        error!("保存角色配置失败: {}", e);
                    } else {
                        info!("角色已归隐，配置已更新: {:?}", char_yaml);
                    }
                }
                Err(e) => {
                    error!("读取角色配置失败: {}", e);
                }
            }
        }
    } else {
        // 死亡角色：配置已经是 Dead 状态，无需修改
        info!("死亡角色已清理本地状态");
    }

    // 8. 触发重连，让主循环重新注册新角色
    if let Some(ref tx) = state.reconnect_tx {
        let server_ws_url = state.server_ws_url.read().await.clone();
        let reconnect_req = super::ReconnectRequest {
            ws_url: server_ws_url,
        };
        if let Err(e) = tx.send(reconnect_req) {
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

/// Compute dream data directory for a specific character.
/// Returns `character_dir / agent_id / data`.
async fn dream_data_dir(state: &HttpApiState, agent_id: uuid::Uuid) -> std::path::PathBuf {
    state
        .character_dir
        .read()
        .await
        .join(agent_id.to_string())
        .join("data")
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
    pub fn load_from_file(data_dir: &std::path::Path, agent_id: &uuid::Uuid) -> Option<Self> {
        if agent_id.is_nil() {
            return None;
        }
        let file_path = data_dir.join(format!("dream_state_{}.json", agent_id));
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
        None
    }

    pub fn save_to_file(&self, data_dir: &std::path::Path, agent_id: &uuid::Uuid) {
        if agent_id.is_nil() {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(data_dir) {
            tracing::error!("创建托梦数据目录失败 {:?}: {}", data_dir, e);
            return;
        }
        let file_path = data_dir.join(format!("dream_state_{}.json", agent_id));
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

    pub fn ensure_loaded(&mut self, data_dir: &std::path::Path, agent_id: &uuid::Uuid) {
        if agent_id.is_nil() {
            return;
        }
        if self.loaded && self.current_agent_id == Some(*agent_id) {
            return;
        }
        if let Some(loaded) = Self::load_from_file(data_dir, agent_id) {
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
        let dd = dream_data_dir(&state, agent_id).await;
        dream.ensure_loaded(&dd, &agent_id);

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
    let dd = dream_data_dir(&state, agent_id).await;
    dream.ensure_loaded(&dd, &agent_id);

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
    dream.save_to_file(&dream_data_dir(&state, agent_id).await, &agent_id);

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
    let dd = dream_data_dir(&state, agent_id).await;
    dream.ensure_loaded(&dd, &agent_id);

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
    let dd = dream_data_dir(&state, agent_id).await;
    dream.ensure_loaded(&dd, &agent_id);

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
    /// 外貌描述
    pub appearance: Option<String>,
    /// 身份
    pub identity: Option<String>,
    /// 性格特征
    pub personality: Vec<String>,
    /// 核心价值观
    pub values: Vec<String>,
    /// 状态 (alive/dead/retired)
    pub status: String,
    /// 所属服务器 URL
    pub server_url: Option<String>,
    /// 注册时间
    pub registered_at: Option<String>,
    /// 是否为当前活跃角色
    pub is_current: bool,
    /// 最近一次连接的现实时间
    pub last_connected_real_time: Option<String>,
    /// 最近一次连接的游戏时间（格式化字符串）
    pub last_connected_world_time: Option<String>,
}

/// 获取所有角色列表
///
/// GET /api/v1/characters
///
/// 返回所有角色（包括已故、归隐的），标记当前活跃角色
pub(super) async fn list_characters_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    // 从文件系统读取所有角色
    let characters = match list_characters_from_fs(&state.character_dir.read().await) {
        Ok(chars) => chars,
        Err(e) => {
            error!("读取角色列表失败: {}", e);
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
    let is_dead = state.is_dead.load(std::sync::atomic::Ordering::Relaxed);
    let current_agent_id = {
        let agent_id = state.agent_id.read().await;
        if agent_id.is_nil() {
            None
        } else {
            Some(agent_id.to_string())
        }
    };

    // 构建角色列表
    let character_infos: Vec<CharacterInfo> = characters
        .iter()
        .map(|c| {
            let is_current = c.agent_id.map(|id| id.to_string()) == current_agent_id;
            // is_dead=true 时当前角色状态应显示为 dead（文件系统可能仍为 Alive）
            let status_override = if is_current && is_dead {
                Some("dead".to_string())
            } else {
                None
            };
            CharacterInfo {
                agent_id: c.agent_id.map(|id| id.to_string()),
                name: c.name.clone(),
                age: c.age,
                gender: c.gender.clone(),
                appearance: c.appearance.clone(),
                identity: c.identity.clone(),
                personality: c.personality.clone(),
                values: c.values.clone(),
                status: status_override.unwrap_or_else(|| match c.status {
                    CharacterStatus::Alive => "alive".to_string(),
                    CharacterStatus::Dead => "dead".to_string(),
                    CharacterStatus::Retired => "retired".to_string(),
                }),
                server_url: c.server_url.clone(),
                registered_at: c.registered_at.map(|t| t.to_rfc3339()),
                is_current,
                last_connected_real_time: c.last_connected_real_time.map(|t| t.to_rfc3339()),
                last_connected_world_time: c
                    .last_connected_world_time
                    .as_ref()
                    .map(|wt| format!("{}年{}月{}日 {}时", wt.year, wt.month, wt.day, wt.hour)),
            }
        })
        .collect();

    Json(CharacterListResponse {
        characters: character_infos,
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

    // 从文件系统查找目标角色
    let characters = match list_characters_from_fs(&state.character_dir.read().await) {
        Ok(chars) => chars,
        Err(e) => {
            error!("读取角色列表失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SwitchCharacterResponse {
                    success: false,
                    message: format!("读取角色列表失败: {}", e),
                    character: None,
                }),
            )
                .into_response();
        }
    };

    let character = match characters.iter().find(|c| c.agent_id == Some(agent_id)) {
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
    if character.status != CharacterStatus::Alive {
        return (
            StatusCode::BAD_REQUEST,
            Json(SwitchCharacterResponse {
                success: false,
                message: format!(
                    "无法切换到{}角色",
                    match character.status {
                        CharacterStatus::Dead => "已故",
                        CharacterStatus::Retired => "归隐",
                        CharacterStatus::Alive => "存活",
                    }
                ),
                character: None,
            }),
        )
            .into_response();
    }

    // 更新内存中的 agent_id 并重置死亡状态
    {
        let mut current_agent_id = state.agent_id.write().await;
        *current_agent_id = agent_id;
    }
    state
        .is_dead
        .store(false, std::sync::atomic::Ordering::Relaxed);

    // 重建 intent_history 以指向新角色的 SQLite 数据库
    {
        let characters_dir = state.character_dir.read().await.clone();
        let data_dir = characters_dir.join(agent_id.to_string()).join("data");
        let new_history = super::intent_history::IntentHistoryStore::open(
            agent_id,
            &data_dir.join(format!("intent_history_{}.db", agent_id)),
        )
        .ok()
        .map(std::sync::Arc::new);
        *state.intent_history.write().await = new_history;
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
            appearance: character.appearance.clone(),
            identity: character.identity.clone(),
            personality: character.personality.clone(),
            values: character.values.clone(),
            status: "alive".to_string(),
            server_url: character.server_url.clone(),
            registered_at: character.registered_at.map(|t| t.to_rfc3339()),
            is_current: true,
            last_connected_real_time: character.last_connected_real_time.map(|t| t.to_rfc3339()),
            last_connected_world_time: character
                .last_connected_world_time
                .as_ref()
                .map(|wt| format!("{}年{}月{}日 {}时", wt.year, wt.month, wt.day, wt.hour)),
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
        runtime_mode: format!("{:?}", state.runtime_mode),
        port: state.actual_port,
    })
}

/// 获取 LLM 停止状态
///
/// GET /api/v1/config/llm-disabled
pub(super) async fn get_llm_disabled_handler(_state: State<HttpApiState>) -> impl IntoResponse {
    // 从全局标志读取状态
    let disabled = crate::component::llm::direct_client::is_llm_disabled();
    Json(serde_json::json!({"llm_disabled": disabled}))
}

/// 设置 LLM 停止状态
///
/// POST /api/v1/config/llm-disabled
pub(super) async fn set_llm_disabled_handler(
    State(state): State<HttpApiState>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let disabled = req
        .get("llm_disabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 立即设置全局标志（立即生效）
    crate::component::llm::direct_client::set_llm_disabled(disabled);

    // 异步保存配置到文件
    let config_path = state.config_path.clone();
    let config_disabled = disabled;
    tokio::spawn(async move {
        let mut config = match crate::config::Config::from_file(&config_path) {
            Ok(c) => c,
            Err(e) => {
                error!("[llm-disabled] 读取配置失败: {}", e);
                return;
            }
        };
        config.runtime.llm_disabled = config_disabled;
        if let Err(e) = config.save_to_file(&config_path) {
            error!("[llm-disabled] 保存配置失败: {}", e);
        }
    });

    if disabled {
        tracing::warn!("[llm-disabled] LLM 调用已停止");
    } else {
        tracing::info!("[llm-disabled] LLM 调用已恢复");
    }

    Json(serde_json::json!({
        "success": true,
        "llm_disabled": disabled,
        "message": if disabled { "LLM 调用已停止" } else { "LLM 调用已恢复" }
    }))
    .into_response()
}

/// 获取动作类型到中文描述的映射
///
/// GET /api/v1/actions - 返回 action_type -> name 映射（短中文名，用于前端展示）
pub(super) async fn get_actions_handler() -> impl IntoResponse {
    let actions = crate::infra::api::cognitive_context::load_available_actions_from_file();
    let map: std::collections::HashMap<String, String> =
        actions.into_iter().map(|a| (a.action, a.name)).collect();
    Json(map)
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
                is_dead: false,
                actual_port: state.actual_port,
            })
            .into_response();
        }
    };

    let has_server = !config.server.ws_url.is_empty();
    let has_llm = config.llm.model.is_some() || config.llm.base_url.is_some();

    // is_dead=true 表示角色已死亡/等待转生，即使文件系统仍为 Alive
    let is_dead = state.is_dead.load(std::sync::atomic::Ordering::Relaxed);

    // 从文件系统检查是否有活跃角色（is_dead 时视为无活跃角色）
    let (has_character, current_character) = if is_dead {
        (false, None)
    } else {
        match get_active_character(&state).await {
            Ok(Some(c)) => (true, Some(c.name.clone())),
            _ => (false, None),
        }
    };

    let needs_setup = !has_server || !has_llm;

    Json(dto::SetupStatusResponse {
        needs_setup,
        has_server,
        has_llm,
        has_character,
        current_character,
        is_dead,
        actual_port: state.actual_port,
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
            runtime_mode: format!("{:?}", state.runtime_mode),
            port: state.actual_port,
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

            // 重建 LLM Client（clone Arc 后立即释放读锁）
            let container = {
                let guard = state.llm_container.read().await;
                guard.clone()
            };
            if let Some(container) = container {
                match crate::component::llm::build_fallback_client(&config.llm) {
                    Ok(new_client) => {
                        *container.write().await = new_client;
                        info!(
                            "[config] LLM Client 已重建: provider={}, model={:?}",
                            config.llm.provider, config.llm.model
                        );
                    }
                    Err(e) => {
                        tracing::error!("[config] LLM Client 重建失败: {}（保留旧实例）", e);
                    }
                }
            }

            let response_config = ConfigResponse {
                server_http_url: config.server.http_url,
                server_ws_url: config.server.ws_url,
                runtime_mode: format!("{:?}", state.runtime_mode),
                port: state.actual_port,
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
    let http_url_value = req
        .http_url
        .clone()
        .unwrap_or_else(|| crate::config::ws_to_http_url(&req.ws_url));

    // 检查是否切换到了不同的服务器
    let server_changed = old_ws_url != req.ws_url;

    // 预计算新服务器目录（server_changed 时两处共用）
    let new_server_dir = state
        .server_dir
        .read()
        .await
        .parent()
        .unwrap_or(state.server_dir.read().await.as_path())
        .to_path_buf()
        .join(crate::config::server_key(&req.ws_url));
    let new_character_dir = new_server_dir.join("characters");

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
        match list_characters_from_fs(&new_character_dir) {
            Ok(all_characters) => {
                // 过滤出该服务器的角色
                let server_characters: Vec<_> = all_characters
                    .iter()
                    .filter(|c| {
                        c.server_url
                            .as_ref()
                            .map(|u| u == &http_url_value)
                            .unwrap_or(false)
                    })
                    .collect();

                previous_characters = server_characters
                    .iter()
                    .map(|c| CharacterSummary {
                        agent_id: c.agent_id.map(|id| id.to_string()).unwrap_or_default(),
                        name: c.name.clone(),
                        status: match c.status {
                            CharacterStatus::Alive => "alive".to_string(),
                            CharacterStatus::Dead => "dead".to_string(),
                            CharacterStatus::Retired => "retired".to_string(),
                        },
                        registered_at: c.registered_at.map(|t| t.to_rfc3339()),
                    })
                    .collect();

                // 检查是否有存活角色
                let has_alive = server_characters
                    .iter()
                    .any(|c| c.status == CharacterStatus::Alive);
                if !has_alive {
                    needs_character_creation = true;
                }
            }
            Err(e) => {
                error!("读取角色列表失败: {}", e);
                needs_character_creation = true;
            }
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

    // 1.5 更新 server_dir 和 character_dir（服务器切换后指向新目录）
    if server_changed {
        *state.server_dir.write().await = new_server_dir;
        *state.character_dir.write().await = new_character_dir;
        info!(
            "[config] server_dir 更新为: {}",
            state.server_dir.read().await.display()
        );
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
        if let Err(e) = tx.send(reconnect_req) {
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
    let openclaw_config_path = crate::component::llm::direct_client::OpenClawConfig::config_path();
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
    use crate::component::llm::direct_client::OpenClawConfig;

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
    use crate::component::llm::{DirectLlmClient, DirectLlmClientConfig, LlmClient, LlmProvider};

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
        fallback_models: config.llm.fallback_models.clone(),
        idle_rotate_threshold: config.llm.idle_rotate_threshold,
        max_consecutive_follow: config.llm.max_consecutive_follow,
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
            fallback_models: Vec::new(),
            idle_rotate_threshold: config.llm.idle_rotate_threshold,
            max_consecutive_follow: config.llm.max_consecutive_follow,
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
    Json(crate::component::llm::snapshot_all_stats())
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
/// 返回引导 OpenClaw LLM 进行按阶段推理的结构化上下文
pub(super) async fn get_cognitive_context_handler(
    State(state): State<HttpApiState>,
) -> impl IntoResponse {
    let current = state.current_state.read().await;

    match current.as_ref() {
        Some(world_state) => {
            let builder = CognitiveContextBuilder::new(Default::default());

            let (persona_info, persona_ref): (
                Option<CognitivePersonaInfo>,
                Option<crate::component::persona::dynamic_persona::DynamicPersona>,
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

// ============================================================================
// LLM Metrics
// ============================================================================

/// GET /api/v1/metrics — LLM 性能指标
pub async fn get_metrics_handler() -> Json<serde_json::Value> {
    use crate::component::llm::snapshot_all_stats;

    let stats = snapshot_all_stats();
    let models: Vec<serde_json::Value> = stats
        .iter()
        .map(|s| {
            let success_rate = if s.calls > 0 {
                (s.calls - s.failures) as f64 / s.calls as f64
            } else {
                1.0
            };
            serde_json::json!({
                "provider": s.provider,
                "model": s.model,
                "calls": s.calls,
                "failures": s.failures,
                "success_rate": format!("{:.0}%", success_rate * 100.0),
                "prompt_tokens": s.prompt_tokens,
                "completion_tokens": s.completion_tokens,
                "total_tokens": s.prompt_tokens + s.completion_tokens,
            })
        })
        .collect();

    Json(serde_json::json!({
        "llm": models,
    }))
}
