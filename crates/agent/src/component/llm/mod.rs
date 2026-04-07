// ============================================================================
// LLM 客户端抽象层
// ============================================================================

mod client;
pub mod direct_client;
mod openai_types;
pub mod token_tracking;
pub mod tool_types;

pub use client::mock;
pub use client::mock::MockLlmClient;
pub use client::{LlmClient, LlmClientExt};
pub use direct_client::{DirectLlmClient, DirectLlmClientConfig, LlmProvider, OpenClawConfig};
pub use token_tracking::{ModelTokenStats, persist_and_reset, record_token_usage, snapshot_all_stats};
pub use tool_types::{ToolCall, ToolDefinition, ToolExecutor};
