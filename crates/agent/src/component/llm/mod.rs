// ============================================================================
// LLM 客户端抽象层
// ============================================================================

mod client;
pub mod direct_client;

pub use client::mock;
pub use client::mock::MockLlmClient;
pub use client::{LlmClient, LlmClientExt};
pub use direct_client::{
    DirectLlmClient, DirectLlmClientConfig, LlmProvider, ModelTokenStats, OpenClawConfig,
    persist_and_reset, record_token_usage, snapshot_all_stats,
};
