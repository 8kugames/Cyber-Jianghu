// ============================================================================
// API请求和响应数据结构
// ============================================================================

use serde::{Deserialize, Serialize};

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

/// Agent注册请求
///
/// POST /api/v1/agent/register 接口的请求数据
#[derive(Debug, Deserialize)]
pub struct AgentRegisterRequest {
    /// Agent名称
    pub name: String,

    /// Agent人设Prompt（LLM使用）
    /// 定义Agent的性格、行为规则等
    /// 注意：根据架构原则，此Prompt应由客户端（Agent SDK）提供
    /// 服务器仅存储，不做任何处理或默认值设置
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// Agent注册响应
///
/// POST /api/v1/agent/register 接口的响应数据
#[derive(Debug, Serialize)]
pub struct AgentRegisterResponse {
    /// Agent唯一ID（UUID）
    pub agent_id: String,

    /// 认证token（WebSocket连接时使用）
    pub auth_token: String,

    /// 注册结果消息
    pub message: String,

    /// 游戏规则（供客户端缓存）
    pub game_rules: GameRules,

    /// 叙事化配置（用于属性描述转换）
    pub narrative_config: protocol::NarrativeConfig,
}
