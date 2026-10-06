// ============================================================================
// 决策模型状态与下载进度端点
// ============================================================================
//
// GET /api/v1/decision-model/status  — 生命周期状态快照（JSON，前端轮询用）
// GET /api/v1/decision-model/events  — 下载/安装进度 SSE 流（复用 sse_util 契约）
//
// SSE 契约：
//   event: connected          data: {"status":"connected"}
//   event: download_progress  data: {file, downloaded_bytes, total_bytes, done}
//   event: disabled           data: {"enabled":false}（功能未开启时一次性推完即收尾）
//   event: heartbeat          data: {}（空闲 30s 一次）

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use http_body::Frame;
use http_body_util::StreamBody;

use super::HttpApiState;
use super::sse_util::{HEARTBEAT_INTERVAL_SECS, sse_frame, sse_response};

/// GET /api/v1/decision-model/status
pub(crate) async fn decision_model_status_handler(State(state): State<HttpApiState>) -> Response {
    axum::Json(status_snapshot(&state).await).into_response()
}

/// 单问超时下限（ms）：低于此值 letter 读出易误判超时
const MIN_TIMEOUT_MS: u64 = 1_000;
/// 单问超时上限（ms）：决策模型路径不受 300s 重试预算约束，过大会拖死整个 tick
const MAX_TIMEOUT_MS: u64 = 120_000;

/// 状态快照：manager 在位时返回运行时状态；停用时回读磁盘配置补齐
/// quant_configured/threshold/timeout_ms（否则面板回填落 HTML 默认值，
/// 下一次保存会把磁盘既有配置静默改写为默认值）。
async fn status_snapshot(state: &HttpApiState) -> serde_json::Value {
    // 先克隆 Arc 再释放槽位读锁，避免 manager.status().await 期间阻塞换装写锁
    let manager = state.decision_model.read().await.clone();
    let Some(manager) = manager else {
        let cfg = crate::config::Config::from_file(&state.config_path)
            .ok()
            .map(|c| c.decision_model);
        return serde_json::json!({
            "enabled": false,
            "quant_configured": cfg.as_ref().map(|c| c.quant.clone()).unwrap_or_default(),
            "threshold": cfg.as_ref().map(|c| c.threshold).unwrap_or_default(),
            "timeout_ms": cfg.as_ref().map(|c| c.timeout_ms).unwrap_or_default(),
            "status": { "state": "disabled" },
        });
    };
    let status = serde_json::to_value(manager.status().await).unwrap_or(serde_json::json!({
        "state": "unknown"
    }));
    serde_json::json!({
        "enabled": manager.is_enabled(),
        "threshold": manager.config().threshold,
        "quant_configured": manager.config().quant,
        "timeout_ms": manager.config().timeout_ms,
        "status": status,
    })
}

/// GET /api/v1/decision-model/events — 下载/安装进度 SSE 流
pub(crate) async fn decision_model_events_handler(State(state): State<HttpApiState>) -> Response {
    let manager = state.decision_model.read().await.clone();
    let Some(manager) = manager else {
        // 未启用：一次性 disabled 事件即收尾（EventSource 自动重连，成本低）
        let stream = futures_util::stream::once(async move {
            Ok::<_, Infallible>(Frame::data(sse_frame("disabled", r#"{"enabled":false}"#)))
        });
        return sse_response(StreamBody::new(stream));
    };
    let mut rx = manager.subscribe_progress();

    let stream = async_stream::stream! {
        yield Ok::<_, Infallible>(Frame::data(sse_frame("connected", r#"{"status":"connected"}"#)));
        loop {
            match tokio::time::timeout(Duration::from_secs(HEARTBEAT_INTERVAL_SECS), rx.recv()).await {
                Ok(Ok(progress)) => {
                    if let Ok(json) = serde_json::to_string(&progress) {
                        yield Ok::<_, Infallible>(Frame::data(sse_frame("download_progress", &json)));
                    }
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped))) => {
                    // 消费端落后：丢弃旧进度帧，通道仍活，继续跟流（不可断连）
                    tracing::warn!("[decision_model] progress broadcast lagged, skipped {skipped}");
                }
                Ok(Err(_)) => break,
                Err(_) => {
                    yield Ok::<_, Infallible>(Frame::data(sse_frame("heartbeat", "{}")));
                }
            }
        }
    };
    sse_response(StreamBody::new(stream))
}

// ============================================================================
// 面板写入口：配置修改（持久化 + 热换装）与手动安装
// ============================================================================

/// POST /api/v1/decision-model/config 请求体（全量字段，面板一次保存）
#[derive(serde::Deserialize)]
pub(crate) struct DecisionModelConfigUpdate {
    pub enabled: bool,
    pub quant: String,
    pub threshold: f32,
    pub timeout_ms: u64,
}

/// POST /api/v1/decision-model/config
///
/// 校验 → 持久化 decision_model 段（读盘→改→原子写盘，沿用 llm config 先例）
/// → 热换装：enabled=true 用新 cfg 构造 manager 原子替换槽位并后台装配，
/// enabled=false 置空槽位（旧 manager drop 时其 llama-server 子进程被 kill_on_drop
/// 回收）。决策回调每 tick 从槽位读取，下一个决策即生效；未就绪期间自动回退
/// 既有 LLM 决策路径，无需重启 agent。
pub(crate) async fn decision_model_config_handler(
    State(state): State<HttpApiState>,
    axum::Json(update): axum::Json<DecisionModelConfigUpdate>,
) -> Response {
    let quant = update.quant.trim().to_ascii_lowercase();
    if !crate::config::DECISION_MODEL_QUANTS.contains(&quant.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "success": false,
                "message": format!("不支持的量化档位: {quant}（可选: {})" , crate::config::DECISION_MODEL_QUANTS.join(", "))
            })),
        )
            .into_response();
    }
    if !(0.0..=1.0).contains(&update.threshold) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "success": false,
                "message": "threshold 必须在 [0, 1] 区间"
            })),
        )
            .into_response();
    }
    if update.timeout_ms < MIN_TIMEOUT_MS || update.timeout_ms > MAX_TIMEOUT_MS {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "success": false,
                "message": format!("timeout_ms 必须在 {MIN_TIMEOUT_MS} - {MAX_TIMEOUT_MS} 之间")
            })),
        )
            .into_response();
    }

    // 持久化（读盘 → 备份 → 改 → 原子写盘）
    let mut config = match crate::config::Config::from_file(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({
                    "success": false,
                    "message": format!("读取配置文件失败: {e}")
                })),
            )
                .into_response();
        }
    };
    let backup = config.clone();
    config.decision_model.enabled = update.enabled;
    config.decision_model.quant = quant;
    config.decision_model.threshold = update.threshold;
    config.decision_model.timeout_ms = update.timeout_ms;
    // 启用时与启动装配同口径：无下载源则拒绝（否则 manager 恒 Failed 且每 tick 回退告警）
    if update.enabled {
        let sources_configured = !config.decision_model.modelscope_repo.trim().is_empty()
            || !config.decision_model.github_release_url.trim().is_empty();
        if !sources_configured {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({
                    "success": false,
                    "message": "modelscope_repo 与 github_release_url 均未配置，无法启用（请在 agent.yaml 配置下载源）"
                })),
            )
                .into_response();
        }
    }
    if let Err(e) = config.save_to_file(&state.config_path) {
        tracing::error!("[decision_model] 保存配置文件失败: {e}");
        if let Err(be) = backup.save_to_file(&state.config_path) {
            tracing::warn!("[decision_model] 备份回写也失败（旧配置保留）: {be}");
        }
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({
                "success": false,
                "message": format!("保存配置失败: {e}")
            })),
        )
            .into_response();
    }

    // 热换装
    if update.enabled {
        let manager = Arc::new(crate::component::decision_model::DecisionModelManager::new(
            config.decision_model.clone(),
            state.decision_model_progress_tx.clone(),
        ));
        let manager_for_install = manager.clone();
        tokio::spawn(async move {
            manager_for_install.install_if_needed().await;
        });
        *state.decision_model.write().await = Some(manager);
        tracing::info!(
            "[decision_model] 面板热换装: quant={} threshold={:.2}",
            config.decision_model.quant,
            config.decision_model.threshold
        );
    } else {
        *state.decision_model.write().await = None;
        tracing::info!("[decision_model] 面板停用决策模型（回退既有 LLM 决策路径）");
    }

    let status = status_snapshot(&state).await;
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "success": true,
            "message": if update.enabled { "决策模型配置已更新并生效" } else { "决策模型已停用，决策走既有 LLM 路径" },
            "config": {
                "enabled": config.decision_model.enabled,
                "quant": config.decision_model.quant,
                "threshold": config.decision_model.threshold,
                "timeout_ms": config.decision_model.timeout_ms,
            },
            "status": status,
        })),
    )
        .into_response()
}

/// POST /api/v1/decision-model/install — 手动触发下载/修复（Failed 或 NotInstalled 后重试）
pub(crate) async fn decision_model_install_handler(State(state): State<HttpApiState>) -> Response {
    let manager = state.decision_model.read().await.clone();
    let Some(manager) = manager else {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({
                "success": false,
                "message": "决策模型未启用，请先在配置中开启"
            })),
        )
            .into_response();
    };
    tokio::spawn(async move {
        if let Err(e) = manager.ensure_installed().await {
            tracing::warn!("[decision_model] 手动安装失败: {e:#}");
        }
    });
    (
        StatusCode::ACCEPTED,
        axum::Json(serde_json::json!({
            "success": true,
            "message": "安装任务已触发，进度见 /api/v1/decision-model/events"
        })),
    )
        .into_response()
}
