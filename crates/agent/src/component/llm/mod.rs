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
pub use client::{FallbackLlmClient, LlmClient, LlmClientExt};
pub use direct_client::{DirectLlmClient, DirectLlmClientConfig, LlmProvider, OpenClawConfig};
pub use token_tracking::{
    ModelTokenStats, persist_and_reset, record_token_usage, snapshot_all_stats,
};
pub use tool_types::{ToolCall, ToolDefinition, ToolExecutor};

use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};

/// 根据 LlmConfig 构建 FallbackLlmClient（含主模型 + fallback 模型）
///
/// 用于启动时和热重载时统一构建逻辑。
pub fn build_fallback_client(llm_config: &crate::config::LlmConfig) -> Result<Arc<dyn LlmClient>> {
    let mut llm_clients: Vec<Arc<dyn LlmClient>> = Vec::new();

    // 主模型
    match build_direct_client(llm_config, llm_config.model.as_deref()) {
        Ok(client) => {
            info!(
                "主模型: {}",
                llm_config.model.as_deref().unwrap_or("default")
            );
            llm_clients.push(Arc::new(client));
        }
        Err(e) => {
            warn!(
                "主模型 ({}) 创建失败: {}",
                llm_config.model.as_deref().unwrap_or("default"),
                e
            );
        }
    }

    // Fallback 模型
    for (i, fallback_model) in llm_config.fallback_models.iter().enumerate() {
        match build_direct_client(llm_config, Some(fallback_model.as_str())) {
            Ok(client) => {
                info!("Fallback 模型 #{}: {}", i + 1, fallback_model);
                llm_clients.push(Arc::new(client));
            }
            Err(e) => {
                warn!(
                    "Fallback 模型 #{} ({}) 创建失败: {}",
                    i + 1,
                    fallback_model,
                    e
                );
            }
        }
    }

    if llm_clients.is_empty() {
        anyhow::bail!("所有 LLM 客户端创建失败（主模型 + fallback 均不可用）");
    }

    let llm_arc: Arc<dyn LlmClient> = if llm_clients.len() > 1 {
        Arc::new(FallbackLlmClient::new(llm_clients))
    } else {
        llm_clients.into_iter().next().unwrap()
    };

    Ok(llm_arc)
}

/// 构建 DirectLlmClient
fn build_direct_client(
    llm_config: &crate::config::LlmConfig,
    model: Option<&str>,
) -> Result<DirectLlmClient> {
    let provider = LlmProvider::parse(&llm_config.provider)
        .ok_or_else(|| anyhow::anyhow!("Unknown LLM provider: {}", llm_config.provider))?;

    let mut client_config = DirectLlmClientConfig::new(provider, llm_config.api_key.clone());

    if let Some(url) = &llm_config.base_url {
        client_config = client_config.with_base_url(url);
    }
    if let Some(m) = model {
        client_config = client_config.with_model(m);
    }
    client_config = client_config
        .with_temperature(llm_config.temperature)
        .with_max_tokens(llm_config.max_tokens);

    DirectLlmClient::new(client_config)
}
