// ============================================================================
// 独立 Embedding HTTP 服务
// ============================================================================
// Docker 部署时作为独立服务运行，agent 通过 HTTP 调用
// 默认端口: 23350，无鉴权（限定本地/Docker 内网）
// ============================================================================

use anyhow::{Context, Result};
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tower_http::cors::CorsLayer;

// ============================================================================
// CLI 参数
// ============================================================================

#[derive(Parser, Debug)]
#[command(name = "cyber-jianghu-embedding", about = "Embedding HTTP 服务")]
struct Args {
    /// 监听端口
    #[arg(long, default_value = "23350", env = "EMBEDDING_PORT")]
    port: u16,

    /// 模型目录
    #[arg(long, env = "EMBEDDING_MODEL_DIR")]
    model_dir: Option<String>,

    /// 模型镜像 URL
    #[arg(long, default_value = "https://hf-mirror.com", env = "EMBEDDING_MIRROR")]
    mirror: String,

    /// 自动下载模型（模型不存在时）
    #[arg(long, default_value = "true", env = "EMBEDDING_AUTO_DOWNLOAD")]
    auto_download: bool,
}

// ============================================================================
// API 类型
// ============================================================================

#[derive(Debug, Deserialize)]
struct EmbedRequest {
    text: String,
}

#[derive(Debug, Serialize)]
struct EmbedResponse {
    embedding: Vec<f32>,
    dimension: usize,
}

#[derive(Debug, Deserialize)]
struct EmbedBatchRequest {
    texts: Vec<String>,
}

#[derive(Debug, Serialize)]
struct EmbedBatchResponse {
    embeddings: Vec<Vec<f32>>,
    dimension: usize,
    count: usize,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: String,
    model_loaded: bool,
    dimension: usize,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

// ============================================================================
// 应用状态
// ============================================================================

struct AppState {
    embedder: Arc<Mutex<Option<cyber_jianghu_embedding::LocalEmbedder>>>,
    config: cyber_jianghu_embedding::LocalEmbedderConfig,
}

// ============================================================================
// HTTP 处理器
// ============================================================================

async fn health_handler(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let loaded = state.embedder.lock().expect("lock poisoned").is_some();
    Json(HealthResponse {
        status: "ok".to_string(),
        model_loaded: loaded,
        dimension: state.config.expected_dim,
    })
}

async fn embed_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EmbedRequest>,
) -> Result<Json<EmbedResponse>, (StatusCode, Json<ErrorResponse>)> {
    let embedder = state.embedder.clone();
    let embedding = tokio::task::spawn_blocking(move || {
        let guard = embedder.lock().expect("lock poisoned");
        match guard.as_ref() {
            Some(e) => e.embed(&req.text),
            None => Err(anyhow::anyhow!("Embedder not loaded")),
        }
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("spawn_blocking panicked: {}", e),
            }),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Embedding failed: {}", e),
            }),
        )
    })?;

    Ok(Json(EmbedResponse {
        dimension: embedding.len(),
        embedding,
    }))
}

async fn embed_batch_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EmbedBatchRequest>,
) -> Result<Json<EmbedBatchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let embedder = state.embedder.clone();
    let result = tokio::task::spawn_blocking(move || {
        let guard = embedder.lock().expect("lock poisoned");
        match guard.as_ref() {
            Some(e) => {
                let refs: Vec<&str> = req.texts.iter().map(|s| s.as_str()).collect();
                e.embed_batch(&refs)
            }
            None => Err(anyhow::anyhow!("Embedder not loaded")),
        }
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("spawn_blocking panicked: {}", e),
            }),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Batch embedding failed: {}", e),
            }),
        )
    })?;

    let count = result.len();
    let dimension = result.first().map(|e| e.len()).unwrap_or(0);
    Ok(Json(EmbedBatchResponse {
        embeddings: result,
        dimension,
        count,
    }))
}

// ============================================================================
// 主入口
// ============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    // 构建模型配置
    let config = match &args.model_dir {
        Some(dir) => cyber_jianghu_embedding::LocalEmbedderConfig::from_dir(dir.into()),
        None => cyber_jianghu_embedding::LocalEmbedderConfig::default(),
    };

    // 检查模型是否存在，不存在则下载
    if !config.is_model_available() {
        if args.auto_download {
            tracing::info!("模型不存在，开始下载: {:?}", config.model_dir);
            cyber_jianghu_embedding::download_model(&config.model_dir, &args.mirror).await?;
        } else {
            anyhow::bail!(
                "模型不存在且未启用自动下载: {:?}",
                config.model_dir
            );
        }
    }

    // 加载模型
    let embedder = cyber_jianghu_embedding::LocalEmbedder::load_with_config(config.clone())
        .context("加载嵌入模型失败")?;
    tracing::info!(
        "模型加载完成，维度: {}",
        embedder.embedding_dim()
    );

    let state = Arc::new(AppState {
        embedder: Arc::new(Mutex::new(Some(embedder))),
        config,
    });

    // 构建路由
    let app = Router::new()
        .route("/api/health", get(health_handler))
        .route("/api/embed", post(embed_handler))
        .route("/api/embed-batch", post(embed_batch_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    // 启动服务
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], args.port));
    tracing::info!("Embedding 服务启动: http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .await
        .context("HTTP 服务运行失败")?;

    Ok(())
}
