//! LLM 客户端接口
//!
//! 支持 LLM 客户端的多种实现：
//! - DirectLlmClient: 直接调用 LLM Provider API（Ollama/OpenClaw/OpenAI Compatible）
//! - MockLlmClient: 测试用 Mock 客户端

mod client;
pub mod direct_client;
// openclaw_client removed (dead code)

pub use client::mock;
pub use client::mock::MockLlmClient;
pub use client::{LlmClient, LlmClientExt};
pub use direct_client::{
    DirectLlmClient, DirectLlmClientConfig, LlmProvider, OpenClawConfig, TokenUsageSnapshot,
    token_usage_tracker,
};
