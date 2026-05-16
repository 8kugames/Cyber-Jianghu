// ============================================================================
// LLM 客户端抽象层
// ============================================================================

mod client;
pub mod conversation;
pub mod direct_client;
mod openai_types;
pub mod streaming;
pub mod token_tracking;
pub mod tool_types;

pub use client::mock;
pub use client::mock::MockLlmClient;
pub use client::{ConversationInput, ConversationTurn, FallbackLlmClient, LlmClient, LlmClientExt};
pub use direct_client::{DirectLlmClient, DirectLlmClientConfig, LlmProvider, OpenClawConfig};
pub use token_tracking::{
    ModelTokenStats, persist_and_reset, record_failure,
    record_token_usage, snapshot_all_stats,
};
pub use tool_types::{ToolCall, ToolDefinition, ToolExecutor};

use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};

/// 根据 LlmConfig 构建 FallbackLlmClient（含主模型 + fallback 模型）
///
/// 用于启动时和热重载时统一构建逻辑。
/// 支持 `models`（per-model 配置）和 `fallback_models`（共享配置）两种格式。
pub fn build_fallback_client(
    llm_config: &crate::config::LlmConfig,
    prefer_stream: bool,
    earth_soul_config: Option<crate::soul::earth::config::EarthSoulConfig>,
) -> Result<Arc<dyn LlmClient>> {
    let mut llm_clients: Vec<Arc<dyn LlmClient>> = Vec::new();

    // 优先使用 models（per-model 配置），否则回退到 fallback_models
    if !llm_config.models.is_empty() {
        for (i, mc) in llm_config.models.iter().enumerate() {
            let max_tokens = mc.max_tokens.unwrap_or(llm_config.max_tokens);
            let enable_thinking = mc.enable_thinking.or(llm_config.enable_thinking);
            match build_direct_client_with_max_tokens(
                llm_config,
                Some(mc.model.as_str()),
                prefer_stream,
                max_tokens,
                enable_thinking,
                earth_soul_config.clone(),
            ) {
                Ok(client) => {
                    info!("模型 #{}: {} (max_tokens={})", i + 1, mc.model, max_tokens);
                    llm_clients.push(Arc::new(client));
                }
                Err(e) => {
                    warn!("模型 #{} ({}) 创建失败: {}", i + 1, mc.model, e);
                }
            }
        }
    } else {
        // 主模型（旧格式 fallback_models）
        match build_direct_client(
            llm_config,
            llm_config.model.as_deref(),
            prefer_stream,
            earth_soul_config.clone(),
        ) {
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
            match build_direct_client(
                llm_config,
                Some(fallback_model.as_str()),
                prefer_stream,
                earth_soul_config.clone(),
            ) {
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
    }

    if llm_clients.is_empty() {
        anyhow::bail!("所有 LLM 客户端创建失败（主模型 + fallback 均不可用）");
    }

    let llm_arc: Arc<dyn LlmClient> = if llm_clients.len() > 1 {
        let mut fb = FallbackLlmClient::new(llm_clients);
        fb = fb.with_idle_threshold(llm_config.idle_rotate_threshold as usize);
        Arc::new(fb)
    } else {
        llm_clients.into_iter().next().unwrap()
    };

    Ok(llm_arc)
}

/// 构建 DirectLlmClient（共享全局 max_tokens + enable_thinking）
fn build_direct_client(
    llm_config: &crate::config::LlmConfig,
    model: Option<&str>,
    prefer_stream: bool,
    earth_soul_config: Option<crate::soul::earth::config::EarthSoulConfig>,
) -> Result<DirectLlmClient> {
    build_direct_client_with_max_tokens(
        llm_config,
        model,
        prefer_stream,
        llm_config.max_tokens,
        llm_config.enable_thinking,
        earth_soul_config,
    )
}

/// 构建 DirectLlmClient（指定 max_tokens + enable_thinking）
fn build_direct_client_with_max_tokens(
    llm_config: &crate::config::LlmConfig,
    model: Option<&str>,
    prefer_stream: bool,
    max_tokens: u32,
    enable_thinking: Option<bool>,
    earth_soul_config: Option<crate::soul::earth::config::EarthSoulConfig>,
) -> Result<DirectLlmClient> {
    let provider = LlmProvider::parse(&llm_config.provider)
        .ok_or_else(|| anyhow::anyhow!("Unknown LLM provider: {}", llm_config.provider))?;

    let mut client_config = DirectLlmClientConfig::new(provider, llm_config.api_key.clone());
    client_config.prefer_stream = prefer_stream;

    if let Some(url) = &llm_config.base_url {
        client_config = client_config.with_base_url(url);
    }
    if let Some(m) = model {
        client_config = client_config.with_model(m);
    }
    client_config = client_config
        .with_temperature(llm_config.temperature)
        .with_max_tokens(max_tokens)
        .with_enable_thinking(enable_thinking);

    let mut client = DirectLlmClient::new(client_config)?;
    if let Some(esc) = earth_soul_config {
        client = client.with_earth_soul_config(esc);
    }
    Ok(client)
}
