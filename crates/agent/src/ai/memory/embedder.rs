// ============================================================================
// 嵌入服务（三层降级）
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 降级策略：
// 1. OpenClaw embed() -> 成功则返回
// 2. bge-small-zh-v1.5 (本地) -> 成功则返回
// 3. 禁用向量记忆，使用 SQLite FTS5 降级
// ============================================================================

use crate::ai::llm::LlmClient;
use crate::ai::memory::local_embedder::LocalEmbedder;
use crate::ai::memory::types::EmbedderStatus;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 嵌入服务
///
/// 管理向量嵌入的生成，支持三层降级
pub struct EmbedderService {
    /// LLM 客户端（OpenClaw）
    llm_client: Option<Arc<dyn LlmClient>>,
    /// 本地嵌入器（降级方案）
    local_embedder: Arc<Mutex<Option<LocalEmbedder>>>,
    /// 当前状态
    status: Arc<Mutex<EmbedderStatus>>,
    /// 是否已初始化
    initialized: AtomicBool,
}

impl EmbedderService {
    /// 创建新的嵌入服务
    pub fn new(llm_client: Option<Arc<dyn LlmClient>>) -> Self {
        Self {
            llm_client,
            local_embedder: Arc::new(Mutex::new(None)),
            status: Arc::new(Mutex::new(EmbedderStatus::Unavailable)),
            initialized: AtomicBool::new(false),
        }
    }

    /// 初始化服务（懒加载本地模型）
    pub fn initialize(&self) -> Result<()> {
        if self.initialized.load(Ordering::SeqCst) {
            return Ok(());
        }

        // 检查是否有 OpenClaw 客户端
        if self.llm_client.is_some() {
            *self.status.lock().unwrap() = EmbedderStatus::OpenClaw;
            self.initialized.store(true, Ordering::SeqCst);
            return Ok(());
        }

        // 尝试加载本地模型
        if LocalEmbedder::is_model_available() {
            match LocalEmbedder::load() {
                Ok(embedder) => {
                    *self.local_embedder.lock().unwrap() = Some(embedder);
                    *self.status.lock().unwrap() = EmbedderStatus::Local;
                    tracing::info!("Local embedder loaded successfully");
                }
                Err(e) => {
                    tracing::warn!("Failed to load local embedder: {}", e);
                    *self.status.lock().unwrap() = EmbedderStatus::Unavailable;
                }
            }
        } else {
            tracing::warn!("Local embedder model not available, vector search disabled");
            *self.status.lock().unwrap() = EmbedderStatus::Unavailable;
        }

        self.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// 生成单个文本的嵌入向量
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.initialize()?;

        let status = *self.status.lock().unwrap();

        match status {
            EmbedderStatus::OpenClaw => {
                // 尝试使用 OpenClaw
                if let Some(client) = &self.llm_client {
                    match self.embed_with_openclaw(client.as_ref(), text).await {
                        Ok(embedding) => return Ok(embedding),
                        Err(e) => {
                            tracing::warn!(
                                "OpenClaw embedding failed: {}, falling back to local",
                                e
                            );
                            // 降级到本地
                            self.fallback_to_local()?;
                        }
                    }
                }
            }
            EmbedderStatus::Local => {
                // 使用本地模型
            }
            EmbedderStatus::Unavailable => {
                return Err(anyhow::anyhow!("Embedder service unavailable"));
            }
        }

        // 使用本地嵌入器
        let guard = self.local_embedder.lock().unwrap();
        if let Some(embedder) = guard.as_ref() {
            embedder.embed(text)
        } else {
            Err(anyhow::anyhow!("Local embedder not available"))
        }
    }

    /// 批量生成嵌入向量
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.initialize()?;

        let status = *self.status.lock().unwrap();

        match status {
            EmbedderStatus::OpenClaw => {
                if let Some(client) = &self.llm_client {
                    // 批量调用 OpenClaw
                    let mut results = Vec::with_capacity(texts.len());
                    for text in texts {
                        match self.embed_with_openclaw(client.as_ref(), text).await {
                            Ok(embedding) => results.push(embedding),
                            Err(e) => {
                                tracing::warn!(
                                    "OpenClaw batch embedding failed: {}, falling back to local",
                                    e
                                );
                                self.fallback_to_local()?;
                                return self.embed_batch_local(texts);
                            }
                        }
                    }
                    return Ok(results);
                }
            }
            EmbedderStatus::Local => {
                return self.embed_batch_local(texts);
            }
            EmbedderStatus::Unavailable => {
                return Err(anyhow::anyhow!("Embedder service unavailable"));
            }
        }

        self.embed_batch_local(texts)
    }

    /// 使用 OpenClaw 生成嵌入
    async fn embed_with_openclaw(&self, _client: &dyn LlmClient, _text: &str) -> Result<Vec<f32>> {
        // TODO: 使用 LlmClient 的 embed 接口
        // 目前 OpenClaw 尚未提供 embedding API，直接返回错误以降级到本地模型
        // 这样可以避免返回无意义的随机向量，保证语义检索的准确性
        Err(anyhow::anyhow!("OpenClaw embedding API not yet available"))
    }

    /// 降级到本地模型
    fn fallback_to_local(&self) -> Result<()> {
        if LocalEmbedder::is_model_available() {
            let embedder = LocalEmbedder::load()?;
            *self.local_embedder.lock().unwrap() = Some(embedder);
            *self.status.lock().unwrap() = EmbedderStatus::Local;
            tracing::info!("Fallback to local embedder");
            Ok(())
        } else {
            *self.status.lock().unwrap() = EmbedderStatus::Unavailable;
            Err(anyhow::anyhow!("Local embedder not available"))
        }
    }

    /// 使用本地模型批量嵌入
    fn embed_batch_local(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let guard = self.local_embedder.lock().unwrap();
        if let Some(embedder) = guard.as_ref() {
            embedder.embed_batch(texts)
        } else {
            Err(anyhow::anyhow!("Local embedder not available"))
        }
    }

    /// 获取当前状态
    pub fn status(&self) -> EmbedderStatus {
        *self.status.lock().unwrap()
    }

    /// 检查服务是否可用
    pub fn is_available(&self) -> bool {
        *self.status.lock().unwrap() != EmbedderStatus::Unavailable
    }

    /// 获取嵌入向量维度
    pub fn embedding_dim(&self) -> usize {
        512 // bge-small-zh-v1.5 固定 512 维
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llm::MockLlmClient;

    #[tokio::test]
    async fn test_embedder_service_creation() {
        let client = Arc::new(MockLlmClient::with_response("0.1 0.2 0.3"));
        let service = EmbedderService::new(Some(client));
        assert!(service.llm_client.is_some());
    }

    #[tokio::test]
    async fn test_is_available_before_init() {
        let service = EmbedderService::new(None);
        assert!(!service.is_available());
    }

    #[tokio::test]
    async fn test_status_unavailable_without_model() {
        // 创建一个没有本地模型的服务
        let service = EmbedderService::new(None);
        // 初始化应该失败（如果没有本地模型）
        // 这里假设测试环境没有下载模型
        let _ = service.initialize();
        // 状态应该是 Unavailable
        assert_eq!(service.status(), EmbedderStatus::Unavailable);
    }

    #[tokio::test]
    async fn test_status_openclaw_with_client() {
        let client = Arc::new(MockLlmClient::with_response("0.1 0.2"));
        let service = EmbedderService::new(Some(client));
        let _ = service.initialize();
        assert_eq!(service.status(), EmbedderStatus::OpenClaw);
    }

    #[tokio::test]
    async fn test_initialize_idempotent() {
        let service = EmbedderService::new(None);
        let _ = service.initialize();
        let _ = service.initialize();
    }

    #[tokio::test]
    async fn test_embed_unavailable_without_model() {
        let service = EmbedderService::new(None);
        let result = service.embed("test").await;
        // 如果没有本地模型，应该返回错误
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_embed_batch_unavailable_without_model() {
        let service = EmbedderService::new(None);
        let result = service.embed_batch(&["test1", "test2"]).await;
        // 如果没有本地模型，应该返回错误
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_embedding_dim() {
        let service = EmbedderService::new(None);
        assert_eq!(service.embedding_dim(), 512);
    }
}
