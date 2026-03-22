//! LLM 客户端接口
//!
//! 支持 LLM 客户端的多种实现：
//! - DirectLlmClient: 直接调用 LLM Provider API（OpenAI、Anthropic 等）
//! - OpenClawLLMClient: 通过 OpenClaw Gateway 调用（OpenAI 兼容接口）
//! - MockLlmClient: 测试用 Mock 客户端

mod client;
mod direct_client;
// openclaw_client removed (dead code)

pub use client::mock;
pub use client::mock::MockLlmClient;
pub use client::{LlmClient, LlmClientExt};
pub use direct_client::{DirectLlmClient, DirectLlmClientConfig, LlmProvider};
