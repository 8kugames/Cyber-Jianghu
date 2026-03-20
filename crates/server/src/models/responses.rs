// ============================================================================
// API请求和响应数据结构
// ============================================================================

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use cyber_jianghu_protocol as protocol;

// Re-export GameRules from protocol
pub use protocol::GameRules;

/// 健康检查响应
///
/// GET /health 接口的响应数据
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    /// 服务状态（"ok"表示正常）
    pub status: String,

    /// 服务版本号
    pub version: String,

    /// Tick周期（秒）
    pub tick_duration_secs: u64,
}

// ============================================================================
// 设备连接 API（Phase 3）
// ============================================================================

/// 设备连接请求
///
/// POST /api/v1/agent/connect 接口的请求数据
/// 客户端首次启动时调用，注册设备身份
#[derive(Debug, Deserialize)]
pub struct AgentConnectRequest {
    /// 设备唯一标识（客户端生成的 UUID v4）
    pub device_id: Uuid,
}

/// 设备连接响应
///
/// POST /api/v1/agent/connect 接口的响应数据
#[derive(Debug, Serialize)]
pub struct AgentConnectResponse {
    /// 认证令牌（用于后续 WebSocket 连接和 API 调用）
    pub auth_token: String,

    /// 结果消息
    pub message: String,
}

// ============================================================================
// 角色注册 API（Phase 4）
// ============================================================================

/// Agent注册请求
///
/// POST /api/v1/agent/register 接口的请求数据
///
/// 设备认证 + 角色信息
#[derive(Debug, Deserialize)]
pub struct AgentRegisterRequest {
    // === 设备认证 ===
    /// 设备唯一标识
    pub device_id: Uuid,
    /// 设备认证令牌（从 /api/v1/agent/connect 获取）
    pub auth_token: String,

    // === 角色基本信息 ===
    /// 角色名称
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
    /// 身份背景（如：江湖游侠、商人、书生）
    #[serde(default)]
    pub identity: Option<String>,

    // === 性格特征 ===
    #[serde(default)]
    pub personality: Vec<String>,

    // === 核心价值观 ===
    #[serde(default)]
    pub values: Vec<String>,

    // === 语言风格 ===
    #[serde(default)]
    pub language_style: LanguageStyleRequest,

    // === 当前目标 ===
    #[serde(default)]
    pub goals: GoalsRequest,

    // === 系统提示词（自动生成或自定义） ===
    /// 自定义系统提示词（可选，如不提供则自动生成）
    #[serde(default)]
    pub system_prompt: Option<String>,
}

fn default_age() -> u8 { 25 }
fn default_gender() -> String { "男".to_string() }

/// 语言风格请求
#[derive(Debug, Deserialize, Default)]
pub struct LanguageStyleRequest {
    /// 语调：豪迈/温和/冷漠/狡黠
    #[serde(default)]
    pub tone: Option<String>,
    /// 说话特点
    #[serde(default)]
    pub speech_patterns: Vec<String>,
}

/// 目标请求
#[derive(Debug, Deserialize, Default)]
pub struct GoalsRequest {
    /// 短期目标
    #[serde(default)]
    pub short_term: Option<String>,
    /// 长远目标
    #[serde(default)]
    pub long_term: Option<String>,
}

/// Agent注册响应
///
/// POST /api/v1/agent/register 接口的响应数据
#[derive(Debug, Serialize)]
pub struct AgentRegisterResponse {
    /// Agent唯一ID（服务器分配的角色ID）
    pub agent_id: String,

    /// 注册结果消息
    pub message: String,

    /// 游戏规则（供客户端缓存）
    pub game_rules: GameRules,

    /// 叙事化配置（用于属性描述转换）
    pub narrative_config: protocol::NarrativeConfig,
}
