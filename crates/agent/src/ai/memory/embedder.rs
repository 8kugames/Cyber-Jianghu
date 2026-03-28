// ============================================================================
// 嵌入服务（本地模型）
// ============================================================================
// 设计文档: (项目根)/docs/superpowers/specs/2025-03-15-semantic-memory-and-forgetting-design.md
//
// 使用 bge-small-zh-v1.5 本地模型生成向量嵌入，支持 CPU/CUDA/Metal。
// 不可用时降级到 SQLite FTS5 全文搜索。
// ============================================================================

use crate::ai::memory::local_embedder::LocalEmbedder;
use crate::ai::memory::types::EmbedderStatus;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 嵌入服务
///
/// 使用本地 bge-small-zh-v1.5 模型生成向量嵌入。
pub struct EmbedderService {
    /// 本地嵌入器
    local_embedder: Arc<Mutex<Option<LocalEmbedder>>>,
    /// 当前状态
    status: Arc<Mutex<EmbedderStatus>>,
    /// 是否已初始化
    initialized: AtomicBool,
}

impl EmbedderService {
    /// 创建新的嵌入服务
    pub fn new() -> Self {
        Self {
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
        if status != EmbedderStatus::Local {
            return Err(anyhow::anyhow!("Embedder service unavailable"));
        }

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
        if status != EmbedderStatus::Local {
            return Err(anyhow::anyhow!("Embedder service unavailable"));
        }

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
        *self.status.lock().unwrap() == EmbedderStatus::Local
    }

    /// 获取嵌入向量维度
    pub fn embedding_dim(&self) -> usize {
        512 // bge-small-zh-v1.5 固定 512 维
    }
}

impl Default for EmbedderService {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_embedder_service_creation() {
        let service = EmbedderService::new();
        assert!(!service.is_available());
    }

    #[tokio::test]
    async fn test_is_available_before_init() {
        let service = EmbedderService::new();
        assert!(!service.is_available());
    }

    #[tokio::test]
    async fn test_status_unavailable_without_model() {
        let service = EmbedderService::new();
        let _ = service.initialize();
        // 状态应该是 Unavailable（测试环境没有本地模型）
        assert_eq!(service.status(), EmbedderStatus::Unavailable);
    }

    #[tokio::test]
    async fn test_initialize_idempotent() {
        let service = EmbedderService::new();
        let _ = service.initialize();
        let _ = service.initialize();
    }

    #[tokio::test]
    async fn test_embed_unavailable_without_model() {
        let service = EmbedderService::new();
        let result = service.embed("test").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_embed_batch_unavailable_without_model() {
        let service = EmbedderService::new();
        let result = service.embed_batch(&["test1", "test2"]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_embedding_dim() {
        let service = EmbedderService::new();
        assert_eq!(service.embedding_dim(), 512);
    }
}
