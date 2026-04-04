// ============================================================================
// DTO (Data Transfer Objects) - HTTP API 请求/响应结构体
// ============================================================================
//
// 集中管理所有 HTTP API 的请求和响应类型定义
// 保持 handlers.rs 专注于 HTTP 处理逻辑

use serde::{Deserialize, Serialize};

// ============================================================================
// 基础端点 DTOs
// ============================================================================

/// Health check 响应
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    /// Agent ID (null 表示尚未注册角色)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub tick_id: Option<i64>,
}

// ============================================================================
// 关系 API DTOs
// ============================================================================

/// 关系更新请求
#[derive(Deserialize)]
pub struct RelationshipUpdateRequest {
    pub target_agent_id: String,
    pub target_name: String,
    pub favorability_delta: Option<i32>,
    pub event_type: Option<String>,
    pub event_description: Option<String>,
    pub event_favorability_delta: Option<i32>,
}

// ============================================================================
// 寿命 API DTOs
// ============================================================================

/// 寿命状态响应
#[derive(Serialize)]
pub struct LifespanResponse {
    pub current_age: u8,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aging_effects: Option<String>,
}

// ============================================================================
// 记忆 API DTOs
// ============================================================================

/// 记忆搜索请求
#[derive(Deserialize)]
pub struct MemorySearchRequest {
    pub query: String,
    pub limit: Option<usize>,
}

/// 记忆存储请求
#[derive(Deserialize)]
pub struct MemoryStoreRequest {
    pub content: String,
    pub importance: Option<f32>,
}

// ============================================================================
// 验证 API DTOs
// ============================================================================

/// Intent 验证请求（数据驱动）
#[derive(Deserialize)]
pub struct ValidateRequest {
    /// 动作类型（任意字符串）
    pub action_type: String,
    /// Agent ID
    pub agent_id: Option<String>,
    /// Tick ID
    pub tick_id: Option<i64>,
    /// 动作数据（JSON）
    pub action_data: Option<serde_json::Value>,
    /// 人设：性别
    pub persona_gender: Option<String>,
    /// 人设：年龄
    pub persona_age: Option<u8>,
    /// 人设：性格特点
    pub persona_personality: Option<Vec<String>>,
    /// 人设：价值观
    pub persona_values: Option<Vec<String>>,
}

/// Intent 验证响应
#[derive(Serialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub reason: Option<String>,
    pub rejection_type: Option<String>,
    pub narrative: Option<String>,
}

// ============================================================================
// Tick 通知 API DTOs
// ============================================================================

/// Tick 状态响应
#[derive(Serialize)]
pub struct TickStatusResponse {
    /// 当前 Tick ID
    pub tick_id: i64,
    /// Agent ID (null 表示尚未注册角色)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// 是否有新的 WorldState（自上次调用后）
    pub has_new_state: bool,
    /// 距离下次 Tick 的预计秒数（如果已知）
    pub seconds_until_next_tick: Option<u64>,
    /// 最后更新时间戳（ISO 8601，系统当前时间）
    pub last_updated_at: String,
    /// 状态的 tick_id（可能与 tick_id 相同，或者没有时为 null）
    pub state_tick_id: Option<i64>,
    /// 状态的最后更新时间戳（ISO 8601）
    pub state_updated_at: Option<String>,
    /// 状态的存在时间（毫秒）
    pub state_age_ms: Option<u64>,
}

// ============================================================================
// LLM 配置 API DTOs
// ============================================================================

/// LLM Provider 信息
#[derive(Debug, Serialize, Deserialize)]
pub struct LlmProviderInfo {
    pub value: String,
    pub label: String,
    pub requires_base_url: bool,
    /// Provider 是否可用
    ///
    /// - `true`: 可选择
    /// - `false`: 禁选（如 OpenClaw 配置文件不存在）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// 禁选原因（当 disabled=true 时显示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

/// Provider 列表响应
#[derive(Debug, Serialize, Deserialize)]
pub struct LlmProvidersResponse {
    pub providers: Vec<LlmProviderInfo>,
}

/// OpenClaw 默认配置响应
///
/// 仅当用户选择 openclaw provider 时请求此接口
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenClawDefaultsResponse {
    /// Gateway URL（从 `~/.openclaw/openclaw.json` 读取）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// 默认模型（OpenClaw 配置中通常没有此字段）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// LLM 配置信息（不含 API Key）
#[derive(Debug, Serialize, Deserialize)]
pub struct LlmConfigInfo {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub has_api_key: bool,
}

/// LLM 配置响应
#[derive(Debug, Serialize, Deserialize)]
pub struct LlmConfigResponse {
    pub actor: LlmConfigInfo,
    pub reflector: Option<LlmConfigInfo>,
    pub reflector_inherits_actor: bool,
    pub runtime_mode: String,
}

/// LLM 配置更新请求
#[derive(Debug, Deserialize)]
pub struct LlmConfigUpdate {
    pub actor: LlmConfigUpdateDetails,
    pub reflector: Option<LlmConfigUpdateDetails>,
    pub reflector_inherits_actor: bool,
}

/// LLM 配置更新详情
#[derive(Debug, Deserialize)]
pub struct LlmConfigUpdateDetails {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: String,
}

// ============================================================================
// 引导状态 API DTOs
// ============================================================================

/// 引导状态响应
#[derive(Debug, Serialize)]
pub struct SetupStatusResponse {
    /// 是否需要引导配置
    pub needs_setup: bool,
    /// 是否有服务器配置
    pub has_server: bool,
    /// 是否有 LLM 配置
    pub has_llm: bool,
    /// 是否有角色
    pub has_character: bool,
    /// 当前角色名（如果有）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_character: Option<String>,
    /// 角色是否已死亡（等待转生）
    pub is_dead: bool,
    /// HTTP API 服务器实际端口
    pub actual_port: u16,
}
