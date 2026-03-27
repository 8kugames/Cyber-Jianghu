// ============================================================================
// OpenClawBridge - LlmClient implementation for Claw mode
// ============================================================================
//
// Sends LLM prompts via WebSocket to OpenClaw and receives responses.
// Part of unified cognitive architecture where Claw and Cognitive modes
// differ only in LLM call location.
// ============================================================================

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, oneshot};
use uuid::Uuid;

use crate::ai::llm::LlmClient;
use crate::runtime::decision::ws::protocol::UpstreamMessage;

/// LLM Client container that supports hot-reload
///
/// Used by Agent and AgentBuilder for LLM client management.
/// Wrapped in `RwLock` to allow runtime LLM client switching.
pub type LlmClientContainer = Arc<RwLock<Arc<dyn LlmClient>>>;

/// Configuration for OpenClawBridge
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// Timeout for LLM requests in seconds
    pub timeout_secs: u64,
    /// Interval for cleanup task in seconds
    pub cleanup_interval_secs: u64,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 30,
            cleanup_interval_secs: 60,
        }
    }
}

/// Pending LLM request waiting for response
struct PendingRequest {
    /// Sender to deliver the response
    tx: oneshot::Sender<Result<String>>,
    /// When the request was created (for stale cleanup)
    created_at: Instant,
}

const CANCELLED_ERROR: &str = "LLM request cancelled (OpenClaw disconnected)";

/// Bridge that sends LLM requests to OpenClaw via WebSocket
///
/// Implements `LlmClient` trait so it can be used anywhere a generic
/// LLM client is needed (cognitive engine, validator, etc.)
pub struct OpenClawBridge {
    /// WebSocket sender to communicate with OpenClaw
    ws_sender: tokio::sync::mpsc::Sender<UpstreamMessage>,
    /// Map of request_id -> pending request
    pending: Arc<RwLock<HashMap<String, PendingRequest>>>,
    /// Configuration
    config: BridgeConfig,
}

impl OpenClawBridge {
    /// Create a new OpenClawBridge
    ///
    /// Spawns a background task to clean up stale pending requests.
    pub fn new(
        ws_sender: tokio::sync::mpsc::Sender<UpstreamMessage>,
        config: BridgeConfig,
    ) -> Self {
        let bridge = Self {
            ws_sender,
            pending: Arc::new(RwLock::new(HashMap::new())),
            config,
        };
        bridge.spawn_cleanup_task();
        bridge
    }

    fn spawn_cleanup_task(&self) {
        let pending = self.pending.clone();
        let timeout = self.config.timeout_secs;

        tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(1));
            loop {
                timer.tick().await;
                let mut pending = pending.write().await;
                let now = Instant::now();
                let stale_threshold = Duration::from_secs(timeout);

                let mut to_remove = Vec::new();
                for (id, req) in pending.iter() {
                    if now.duration_since(req.created_at) > stale_threshold {
                        to_remove.push(id.clone());
                    }
                }
                for id in to_remove {
                    if let Some(req) = pending.remove(&id) {
                        let _ = req.tx.send(Err(anyhow::anyhow!("{}", CANCELLED_ERROR)));
                    }
                }
            }
        });
    }

    /// Handle an LLM response received from OpenClaw
    ///
    /// Called by the WebSocket server when it receives an LLM response.
    /// Completes the pending request by sending the result through the oneshot channel.
    pub fn handle_response(&self, request_id: &str, content: Result<String>) {
        let pending = self.pending.clone();
        let request_id = request_id.to_string();

        tokio::spawn(async move {
            if let Some(req) = pending.write().await.remove(&request_id) {
                let _ = req.tx.send(content);
            }
        });
    }
}

#[async_trait]
impl LlmClient for OpenClawBridge {
    /// Send an LLM completion request to OpenClaw
    ///
    /// Creates a pending request, sends it via WebSocket, and waits for response.
    /// Times out after `config.timeout_secs` seconds.
    async fn complete(&self, prompt: &str) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        let request_id = Uuid::new_v4().to_string();

        self.pending.write().await.insert(
            request_id.clone(),
            PendingRequest {
                tx,
                created_at: Instant::now(),
            },
        );

        self.ws_sender
            .send(UpstreamMessage::LLMRequest {
                request_id: request_id.clone(),
                prompt: prompt.to_string(),
            })
            .await
            .context("WebSocket sender closed (OpenClaw disconnected)")?;

        match tokio::time::timeout(Duration::from_secs(self.config.timeout_secs), rx).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => Err(anyhow::anyhow!("{}: {}", CANCELLED_ERROR, e)),
            Err(_) => Err(anyhow::anyhow!(
                "timeout after {}s",
                self.config.timeout_secs
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_bridge_config_default() {
        let config = BridgeConfig::default();
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.cleanup_interval_secs, 60);
    }

    #[tokio::test]
    async fn test_handle_response_completes_request() {
        let (tx, _rx) = mpsc::channel(1);
        let config = BridgeConfig::default();
        let bridge = OpenClawBridge::new(tx, config);

        // Manually insert a pending request
        let (response_tx, response_rx) = oneshot::channel();
        bridge.pending.write().await.insert(
            "test-id".to_string(),
            PendingRequest {
                tx: response_tx,
                created_at: Instant::now(),
            },
        );

        // Handle response
        bridge.handle_response("test-id", Ok("test response".to_string()));

        // Verify response received
        let result = response_rx.await.unwrap();
        assert_eq!(result.unwrap(), "test response".to_string());

        // Verify pending request removed
        assert!(bridge.pending.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_handle_response_unknown_request_ignored() {
        let (tx, _rx) = mpsc::channel(1);
        let config = BridgeConfig::default();
        let bridge = OpenClawBridge::new(tx, config);

        // Handle response for non-existent request (should not panic)
        bridge.handle_response("unknown-id", Ok("test".to_string()));

        // Give the spawned task time to complete
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
