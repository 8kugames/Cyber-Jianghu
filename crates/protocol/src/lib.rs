//! # Cyber-Jianghu Protocol
//!
//! 定义 Server 和 Agent 之间的通信协议。
//!
//! ## 核心类型
//!
//! - [`ServerMessage`] - 服务端下发的消息
//! - [`ClientMessage`] - 客户端上报的消息
//! - [`WorldState`] - 世界状态快照
//! - [`Intent`] - Agent 意图
//!
//! ## 使用示例
//!
//! ```rust
//! use cyber_jianghu_protocol::{ServerMessage, ClientMessage, Intent};
//! use cyber_jianghu_protocol::ActionType;
//! use uuid::Uuid;
//!
//! // 创建意图
//! let intent = Intent::new(
//!     Uuid::new_v4(),
//!     1,
//!     "说话",
//!     Some(serde_json::json!({"content": "Hello World"})),
//! );
//! ```
//!
//! # Features
//!
//! - `sqlx-support`: 启用 sqlx 数据库类型支持（仅服务端需要）

pub mod error;
pub mod messages;
pub mod resolve;
pub mod types;

// 可选的 sqlx 类型支持
#[cfg(feature = "sqlx-support")]
pub mod sqlx_types;

// 重导出常用类型
pub use messages::{
    ClientMessage, DialogueMessage, DialogueSession, EarthToolCall, FinalIntentReport,
    ImmediateIntentReport, LayerReport, PipelineAction, RenhunReport, ServerMessage,
    SoulCycleAttempt, SoulCycleMetadata, TianhunReport,
};
pub use types::*;

// 重导出错误类型（从 common 合并）
pub use error::GameError;

// 重导出 agent ID 解析工具
pub use resolve::{ResolveAgentIdError, resolve_agent_id, resolve_agent_id_lenient, short_id};

/// 协议版本
pub const PROTOCOL_VERSION: &str = env!("CARGO_PKG_VERSION");

// ============================================================================
// LLM 配置默认值（agent + server 共享唯一来源）
// ============================================================================
//
// 所有 LLM/agent 相关默认常量集中此处,避免 agent/src/config.rs 与
// server/src/{handlers,loaders} 重复定义。env var 优先于 const 优先于
// 调用方的 YAML 覆盖。
//
// 命名约定: CYBER_JIANGHU_<DOMAIN>_<FIELD> (全大写,下划线)

/// LLM 输出 token 默认上限
///
/// 所有 LLM 相关 max_tokens 字段的单一来源,包括:
/// - LlmConfig.max_tokens (agent 默认配置)
/// - DirectLlmClientConfig::new() 默认值
/// - LlmClient::retry_max_tokens_baseline/ceiling default impl 派生
/// - Server LlmConfig (config_llm handler + llm_loader) 默认值
pub const DEFAULT_LLM_MAX_TOKENS: u32 = 8192;

/// LLM provider 默认值
pub const DEFAULT_LLM_PROVIDER: &str = "ollama";

/// LLM 温度默认
pub const DEFAULT_LLM_TEMPERATURE: f32 = 0.7;

/// 连续 idle tick 达到此阈值后主动切换下一个模型
pub const DEFAULT_IDLE_ROTATE_THRESHOLD: u32 = 24;

/// 模型上下文窗口默认大小（tokens）
///
/// agent LlmConfig + server LlmConfig + 任何上下文窗口相关字段的单一来源。
pub const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 32768;

/// Summary 触发比例（0.0-1.0），token 数超过此比例时触发压缩
pub const DEFAULT_SUMMARY_TRIGGER_RATIO: f64 = 0.75;

/// Summary 后保留最近 N 轮对话
pub const DEFAULT_KEEP_RECENT_TURNS: u32 = 4;

/// Agent ↔ Server 重连延迟（秒）
pub const DEFAULT_RECONNECT_DELAY_SECS: u64 = 5;

/// 等待执行结果超时（毫秒）
pub const DEFAULT_EXECUTION_RESULT_TIMEOUT_MS: u64 = 3000;

/// 灵魂周期上报重试次数
pub const DEFAULT_SOUL_CYCLE_REPORT_RETRIES: u32 = 3;

/// 灵魂周期上报基础延迟（毫秒），指数退避
pub const DEFAULT_SOUL_CYCLE_REPORT_BASE_DELAY_MS: u64 = 100;

/// NarrativeSummaryWindow 窗口大小
pub const DEFAULT_NARRATIVE_WINDOW_SIZE: usize = 5;

/// 语义去重历史窗口大小
pub const DEFAULT_SEMANTIC_DEDUP_HISTORY: usize = 1;

/// 启用 SSE 流式 LLM 调用
pub const DEFAULT_ENABLE_STREAMING: bool = false;

// ============================================================================
// 事件类型常量
// ============================================================================

/// 世界状态事件类型
pub const EVENT_TYPE_WORLD_STATE: &str = "world_state";
/// 动作结果事件类型
pub const EVENT_TYPE_ACTION_RESULT: &str = "action_result";
/// 状态变化事件类型
pub const EVENT_TYPE_STATE_CHANGE: &str = "state_change";
/// 公开消息事件类型
pub const EVENT_TYPE_PUBLIC_MESSAGE: &str = "public_message";
/// 系统通知事件类型
pub const EVENT_TYPE_SYSTEM_NOTIFICATION: &str = "system_notification";
/// 死亡通知事件类型
pub const EVENT_TYPE_DEATH_NOTIFICATION: &str = "death_notification";
/// 环境变化事件类型
pub const EVENT_TYPE_ENVIRONMENTAL_CHANGE: &str = "environmental_change";
/// 社交互动事件类型
pub const EVENT_TYPE_SOCIAL_INTERACTION: &str = "social_interaction";

// ============================================================================
// 服务端错误码
// ============================================================================

/// Tick 不匹配（意图的 tick_id 与服务端当前 tick 不一致）
pub const ERROR_CODE_TICK_MISMATCH: &str = "tick_mismatch";
/// 服务端尚未开始接受意图
pub const ERROR_CODE_NOT_ACCEPTING: &str = "not_accepting";
/// Agent 已死亡
pub const ERROR_CODE_AGENT_DEAD: &str = "agent_dead";
/// 速率限制
pub const ERROR_CODE_RATE_LIMITED: &str = "rate_limited";
/// 无效消息格式
pub const ERROR_CODE_INVALID_MESSAGE: &str = "invalid_message";
/// 对话失败
pub const ERROR_CODE_DIALOGUE_FAILED: &str = "dialogue_failed";
/// 动作处理失败（通用）
pub const ERROR_CODE_ACTION_FAILED: &str = "action_failed";
