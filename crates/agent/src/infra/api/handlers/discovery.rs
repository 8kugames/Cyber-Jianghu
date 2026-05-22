// API 发现端点
// ============================================================================


use axum::{
    extract::State,
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};

use super::HttpApiState;

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
pub(crate) async fn api_list_handler(State(state): State<HttpApiState>) -> impl IntoResponse {
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
                "action_type": "说话",
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
            path: "/api/v1/memory/daily-summaries".to_string(),
            method: "GET".to_string(),
            description: "获取每日摘要记忆".to_string(),
            request_example: None,
            response_example: Some(serde_json::json!({
                "summaries": [
                    { "id": 1, "tick_id": 123, "content": "游戏日 1 摘要...", "importance": 0.8, "created_at": "2026-04-29T12:00:00Z" }
                ],
                "count": 1
            })),
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
                "action_type": "攻击",
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
                "intent": {"action_type": "攻击", "action_data": null},
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
