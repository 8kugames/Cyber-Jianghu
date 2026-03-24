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
