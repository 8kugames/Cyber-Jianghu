// ============================================================================
// 嵌入服务（本地 / 远程 / 降级）
// ============================================================================
// 支持三种 provider:
// - Local: 进程内 candle-transformers（单 agent 进程部署）
// - Remote: HTTP 调用独立 embedding 服务（Docker 多 agent 部署）
// - None: FTS5 降级
// ============================================================================

use crate::component::memory::local_embedder::LocalEmbedder;
use crate::component::memory::types::EmbedderStatus;
use anyhow::{Context, Result};
use std::sync::{Arc, Mutex, OnceLock};

/// bge-small-zh-v1.5 模型固定输出维度
const BGE_SMALL_ZH_DIM: usize = 512;

/// 嵌入服务配置
#[derive(Debug, Clone, Default)]
pub struct EmbedderServiceConfig {
    /// 远程 embedding 服务 URL
    /// 设定后优先使用远程服务，连接失败时 fast fail（不静默降级到本地）
    pub remote_url: Option<String>,
}

/// 嵌入服务
pub struct EmbedderService {
    local_embedder: Arc<Mutex<Option<LocalEmbedder>>>,
    status: OnceLock<EmbedderStatus>,
    config: EmbedderServiceConfig,
    http_client: Option<reqwest::Client>,
    embed_base_url: Option<String>,
}

impl EmbedderService {
    pub fn new() -> Self {
        let remote_url = std::env::var("CYBER_JIANGHU_EMBEDDER_REMOTE_URL").ok();
        let config = EmbedderServiceConfig { remote_url };
        Self::with_config(config)
    }

    pub fn with_config(config: EmbedderServiceConfig) -> Self {
        let http_client = config.remote_url.as_ref().map(|url| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|e| {
                    panic!(
                        "embedding HTTP 客户端构建失败 (target: {}): {} — 请检查 TLS 后端配置",
                        url, e
                    )
                })
        });

        let embed_base_url = config
            .remote_url
            .as_ref()
            .map(|url| url.trim_end_matches('/').to_string());

        Self {
            local_embedder: Arc::new(Mutex::new(None)),
            status: OnceLock::new(),
            config,
            http_client,
            embed_base_url,
        }
    }

    /// 异步初始化（OnceLock 保证只执行一次，无 TOCTOU 竞态）
    async fn ensure_initialized(&self) -> Result<EmbedderStatus> {
        if let Some(&status) = self.status.get() {
            return Ok(status);
        }

        let status = if self.config.remote_url.is_some() {
            self.init_remote().await?
        } else {
            self.init_local()?
        };

        // OnceLock::set 只会成功一次，多线程安全
        let _ = self.status.set(status);
        Ok(status)
    }

    /// 远程初始化：显式配置了 remote_url 时使用
    /// 连接失败 → fast fail，不静默降级
    async fn init_remote(&self) -> Result<EmbedderStatus> {
        let client = self
            .http_client
            .as_ref()
            .context("HTTP 客户端未初始化（remote_url 已配置）")?;

        let base_url = self
            .embed_base_url
            .as_ref()
            .context("embed_base_url 未设置")?;

        let health_url = format!("{}/api/health", base_url);

        let resp = client
            .get(&health_url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .with_context(|| {
                format!(
                    "远程 embedding 服务连接失败: {} (请检查服务是否启动)",
                    base_url
                )
            })?;

        if resp.status().is_success() {
            tracing::info!("远程 embedding 服务已连接: {}", base_url);
            Ok(EmbedderStatus::Remote)
        } else {
            anyhow::bail!(
                "远程 embedding 服务健康检查失败: HTTP {} (url: {})",
                resp.status(),
                base_url
            )
        }
    }

    /// 本地初始化：无 remote_url 时使用
    fn init_local(&self) -> Result<EmbedderStatus> {
        let config = cyber_jianghu_embedding::LocalEmbedderConfig::default_path();

        if !config.is_model_available() {
            tracing::warn!("本地嵌入模型不存在: {:?}", config.model_dir);
            return Ok(EmbedderStatus::Unavailable);
        }

        match LocalEmbedder::load_with_config(config) {
            Ok(embedder) => {
                *self.local_embedder.lock().expect("lock poisoned") = Some(embedder);
                tracing::info!("本地嵌入模型加载成功");
                Ok(EmbedderStatus::Local)
            }
            Err(e) => {
                tracing::error!("本地嵌入模型加载失败: {} (路径可能损坏)", e);
                Ok(EmbedderStatus::Unavailable)
            }
        }
    }

    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let status = self.ensure_initialized().await?;

        match status {
            EmbedderStatus::Local => self.embed_local(text).await,
            EmbedderStatus::Remote => self.embed_remote(text).await,
            EmbedderStatus::Unavailable => Err(anyhow::anyhow!("Embedder service unavailable")),
        }
    }

    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let status = self.ensure_initialized().await?;

        match status {
            EmbedderStatus::Local => self.embed_batch_local(texts).await,
            EmbedderStatus::Remote => self.embed_batch_remote(texts).await,
            EmbedderStatus::Unavailable => Err(anyhow::anyhow!("Embedder service unavailable")),
        }
    }

    // ========================================================================
    // Local provider
    // ========================================================================

    async fn embed_local(&self, text: &str) -> Result<Vec<f32>> {
        let embedder = self.local_embedder.clone();
        let text = text.to_owned();
        tokio::task::spawn_blocking(move || {
            let guard = embedder.lock().expect("lock poisoned");
            match guard.as_ref() {
                Some(e) => e.embed(&text),
                None => Err(anyhow::anyhow!("Local embedder not available")),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking panicked: {}", e))?
    }

    async fn embed_batch_local(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let embedder = self.local_embedder.clone();
        let texts: Vec<String> = texts.iter().map(|s| (*s).to_owned()).collect();
        tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            let guard = embedder.lock().expect("lock poisoned");
            match guard.as_ref() {
                Some(e) => e.embed_batch(&refs),
                None => Err(anyhow::anyhow!("Local embedder not available")),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking panicked: {}", e))?
    }

    // ========================================================================
    // Remote provider
    // ========================================================================

    async fn embed_remote(&self, text: &str) -> Result<Vec<f32>> {
        let client = self.http_client.as_ref().context("HTTP 客户端未初始化")?;
        let base_url = self
            .embed_base_url
            .as_ref()
            .context("embed_base_url 未设置")?;
        let url = format!("{}/api/embed", base_url);

        let resp = client
            .post(&url)
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await
            .with_context(|| format!("远程 embedding 请求失败: {}", url))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("远程 embedding 返回错误 {}: {}", status, body);
        }

        let result: serde_json::Value = resp.json().await.context("解析远程 embedding 响应失败")?;

        parse_embedding_response(&result, "embedding")
    }

    async fn embed_batch_remote(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let client = self.http_client.as_ref().context("HTTP 客户端未初始化")?;
        let base_url = self
            .embed_base_url
            .as_ref()
            .context("embed_base_url 未设置")?;
        let url = format!("{}/api/embed-batch", base_url);

        let resp = client
            .post(&url)
            .json(&serde_json::json!({ "texts": texts }))
            .send()
            .await
            .with_context(|| format!("远程 batch embedding 请求失败: {}", url))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("远程 batch embedding 返回错误 {}: {}", status, body);
        }

        let result: serde_json::Value = resp
            .json()
            .await
            .context("解析远程 batch embedding 响应失败")?;

        parse_batch_response(&result)
    }

    // ========================================================================
    // 状态查询
    // ========================================================================

    pub fn status(&self) -> EmbedderStatus {
        self.status
            .get()
            .copied()
            .unwrap_or(EmbedderStatus::Unavailable)
    }

    pub fn is_available(&self) -> bool {
        self.status() != EmbedderStatus::Unavailable
    }

    /// 维度来自模型规格（bge-small-zh-v1.5 = 512 维）
    pub fn embedding_dim(&self) -> usize {
        BGE_SMALL_ZH_DIM
    }
}

impl Default for EmbedderService {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// JSON 响应解析
// ============================================================================

fn parse_embedding_response(value: &serde_json::Value, field: &str) -> Result<Vec<f32>> {
    value
        .get(field)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect::<Vec<f32>>()
        })
        .filter(|v| !v.is_empty())
        .with_context(|| format!("远程 embedding 响应格式错误 (缺少 {} 字段)", field))
}

fn parse_batch_response(value: &serde_json::Value) -> Result<Vec<Vec<f32>>> {
    value
        .get("embeddings")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|vec_val| {
                    vec_val.as_array().map(|vec| {
                        vec.iter()
                            .filter_map(|v| v.as_f64().map(|f| f as f32))
                            .collect::<Vec<f32>>()
                    })
                })
                .collect::<Vec<Vec<f32>>>()
        })
        .filter(|v| !v.is_empty())
        .context("远程 batch embedding 响应格式错误 (缺少 embeddings 字段)")
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_is_available_before_init() {
        let service = EmbedderService::new();
        assert!(!service.is_available());
    }

    #[tokio::test]
    async fn test_status_unavailable_without_model() {
        let service = EmbedderService::new();
        let _ = service.ensure_initialized().await;
        assert_eq!(service.status(), EmbedderStatus::Unavailable);
    }

    #[tokio::test]
    async fn test_embed_unavailable_without_model() {
        let service = EmbedderService::new();
        let result = service.embed("test").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_embedding_dim() {
        let service = EmbedderService::new();
        assert_eq!(service.embedding_dim(), 512);
    }

    #[test]
    fn test_config_default_no_remote() {
        let config = EmbedderServiceConfig::default();
        assert!(config.remote_url.is_none());
    }

    #[test]
    fn test_config_with_remote_url() {
        let config = EmbedderServiceConfig {
            remote_url: Some("http://localhost:23350".to_string()),
        };
        assert_eq!(config.remote_url.unwrap(), "http://localhost:23350");
    }

    #[test]
    fn test_remote_status_exists() {
        let status = EmbedderStatus::Remote;
        assert_eq!(format!("{:?}", status), "Remote");
    }

    #[test]
    fn test_parse_embedding_response_valid() {
        let json = serde_json::json!({"embedding": [0.1, 0.2, 0.3], "dimension": 3});
        let result = parse_embedding_response(&json, "embedding").unwrap();
        assert_eq!(result, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn test_parse_embedding_response_missing_field() {
        let json = serde_json::json!({"other": []});
        let result = parse_embedding_response(&json, "embedding");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_batch_response_valid() {
        let json = serde_json::json!({"embeddings": [[0.1, 0.2], [0.3, 0.4]], "count": 2});
        let result = parse_batch_response(&json).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_base_url_trimmed() {
        let config = EmbedderServiceConfig {
            remote_url: Some("http://localhost:23350/".to_string()),
        };
        let service = EmbedderService::with_config(config);
        assert_eq!(service.embed_base_url.unwrap(), "http://localhost:23350");
    }

    #[test]
    fn test_bge_dim_constant() {
        assert_eq!(BGE_SMALL_ZH_DIM, 512);
    }
}
