//! Admin 静态文件服务与 token 文件写入（自 main.rs 外移）

use super::*;
use anyhow::Context;
use axum::body::Body;
use axum::http::StatusCode;
use cyber_jianghu_server::db::DbPool;
use cyber_jianghu_server::governance::{
    ActionEvolutionConfig, ProposalStore, SoulReviewEngine, TopicClassifier,
};
use cyber_jianghu_server::state::GovernanceState;
use cyber_jianghu_server::tick::TickScheduler;
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

pub(crate) fn serve_admin_file(path: &str) -> Result<axum::response::Response<Body>, StatusCode> {
    let static_dir =
        match std::fs::canonicalize(cyber_jianghu_server::paths::get_static_dir().join("admin")) {
            Ok(dir) => dir,
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        };

    let file_path = if path.is_empty() || path == "index.html" {
        static_dir.join("index.html")
    } else {
        static_dir.join(path)
    };

    let resolved_path = match std::fs::canonicalize(&file_path) {
        Ok(p) => p,
        Err(_) => return Err(StatusCode::NOT_FOUND),
    };

    if !resolved_path.starts_with(&static_dir) {
        return Err(StatusCode::FORBIDDEN);
    }

    if !resolved_path.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }

    let mime = mime_guess::from_path(&resolved_path).first_or_octet_stream();
    let body =
        Body::from(std::fs::read(&resolved_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?);
    // 管理面板资源不带内容哈希，且运行时按请求读盘：禁用启发式缓存，
    // 避免部署后浏览器复用旧 JS，造成"服务端已更新、面板仍旧行为"的误判。
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, mime.as_ref())
        .header(axum::http::header::CACHE_CONTROL, "no-cache")
        .body(body)
        .unwrap_or_else(|_| axum::response::Response::new(Body::empty())))
}

pub(crate) fn render_admin_token_file_content(
    read_token_source: &str,
    admin_read_token: &str,
    write_token_source: &str,
    admin_write_token: &str,
) -> String {
    format!(
        "========================================\n\
Cyber-Jianghu 管理员访问凭证\n\
========================================\n\
Read Token (只读): [{read_token_source}]\n\
  {admin_read_token}\n\
Write Token (读写): [{write_token_source}]\n\
  {admin_write_token}\n\
\n\
========================================\n"
    )
}

#[cfg(unix)]
pub(crate) fn ensure_admin_token_permissions(path: &Path) -> Result<()> {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, Permissions::from_mode(0o600))
        .with_context(|| format!("设置admin token文件权限失败: {}", path.display()))?;
    let mode = std::fs::metadata(path)
        .with_context(|| format!("读取admin token文件元数据失败: {}", path.display()))?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o600 {
        anyhow::bail!(
            "admin token文件权限异常: {} 实际为 {:o}，预期 600",
            path.display(),
            mode
        );
    }

    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn ensure_admin_token_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

pub(crate) fn write_admin_token_file(
    path: &Path,
    read_token_source: &str,
    admin_read_token: &str,
    write_token_source: &str,
    admin_write_token: &str,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建admin token目录失败: {}", parent.display()))?;
    }

    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;

        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .with_context(|| {
                format!(
                    "无法创建admin token文件 {}: {}",
                    path.display(),
                    "open failed"
                )
            })?
    };

    #[cfg(not(unix))]
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .with_context(|| format!("无法创建admin token文件: {}", path.display()))?;

    ensure_admin_token_permissions(path)?;

    let content = render_admin_token_file_content(
        read_token_source,
        admin_read_token,
        write_token_source,
        admin_write_token,
    );
    use std::io::Write;
    file.write_all(content.as_bytes())
        .with_context(|| format!("写入admin token文件失败: {}", path.display()))?;
    file.flush()
        .with_context(|| format!("刷新admin token文件失败: {}", path.display()))?;

    Ok(())
}

/// /admin/ → serve index.html (no path parameter to extract)
pub(crate) async fn serve_admin_index() -> Result<axum::response::Response<Body>, StatusCode> {
    serve_admin_file("index.html")
}

/// /admin/{*path} → serve the specific file
pub(crate) async fn serve_admin(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Result<axum::response::Response<Body>, StatusCode> {
    serve_admin_file(&path)
}

/// 启动Tick引擎（后台任务）
///
/// Tick引擎在独立的tokio任务中运行，负责驱动游戏世界
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_tick_engine(
    game_data_cache: Arc<cyber_jianghu_server::game_data::GameDataCache>,
    db_pool: DbPool,
    connection_manager: cyber_jianghu_server::websocket::ConnectionManager,
    agent_to_device_map: cyber_jianghu_server::websocket::AgentToDeviceMap,
    worker_tx: tokio::sync::mpsc::Sender<cyber_jianghu_server::tick::WorkerMessage>,
    agent_state_cache: cyber_jianghu_server::state::AgentStateCache,
    accepting_tick_id: Arc<AtomicI64>,
    vendor_pending_events: cyber_jianghu_server::models::VendorPendingEvents,
    prompt_template_cache: Arc<
        tokio::sync::RwLock<Option<cyber_jianghu_server::state::PromptTemplateCache>>,
    >,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick_scheduler = TickScheduler::new(
            game_data_cache,
            db_pool,
            connection_manager,
            agent_to_device_map,
            worker_tx,
            agent_state_cache,
            accepting_tick_id,
            vendor_pending_events,
        );
        tick_scheduler.set_prompt_template_cache(prompt_template_cache);

        // 启动前预加载 prompt_templates 到缓存，确保首个 Agent 连接时即可下发
        if let Err(e) = tick_scheduler.preload_prompt_templates().await {
            warn!("启动时预加载 prompt_templates 失败: {}", e);
        }

        info!("启动Tick引擎（后台任务）");

        if let Err(e) = tick_scheduler.run().await {
            error!("Tick引擎运行失败: {}", e);
        }
    })
}

/// 治理轮询任务的优雅关闭句柄
pub(crate) struct GovernanceShutdown {
    /// 关闭信号发送端：调用 `send(true)` 通知任务退出
    pub(crate) shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// 任务 JoinHandle：在 shutdown 时 await，等待任务收尾
    pub(crate) handle: JoinHandle<()>,
}

/// 初始化治理系统
///
/// 加载 action_evolution.yaml 配置，创建 TopicClassifier、ProposalStore、SoulReviewEngine，
/// 并启动周期审议后台任务。返回 `(GovernanceState, GovernanceShutdown)`，
/// 调用方负责在关闭时通过 `GovernanceShutdown` 优雅停掉轮询任务。
pub(crate) async fn init_governance(
    db_pool: &DbPool,
    connection_manager: cyber_jianghu_server::websocket::ConnectionManager,
    game_data_cache: Arc<cyber_jianghu_server::game_data::GameDataCache>,
) -> Result<(GovernanceState, GovernanceShutdown)> {
    let config_dir = cyber_jianghu_server::paths::get_config_dir();

    // 加载 action_evolution.yaml
    let ae_path = config_dir.join("action_evolution.yaml");
    let ae_content =
        std::fs::read_to_string(&ae_path).context("读取 action_evolution.yaml 失败")?;
    let ae_outer: serde_json::Value =
        serde_yaml::from_str(&ae_content).context("解析 action_evolution.yaml 失败")?;
    let ae_data = ae_outer
        .get("data")
        .context("action_evolution.yaml 缺少 data 字段")?;
    let action_evo_config: ActionEvolutionConfig =
        serde_json::from_value(ae_data.clone()).context("反序列化 ActionEvolutionConfig 失败")?;
    info!("action_evolution.yaml 加载完成");

    let classifier = Arc::new(TopicClassifier::new(action_evo_config.topic_classifier));
    let proposal_store = Arc::new(ProposalStore::new(db_pool.clone()));

    // SoulReviewEngine::load 接受 config_dir，内部加载 souls.yaml
    // 内部 capability_manifest 已是 Arc<RwLock<...>>，外层无需再加锁
    let engine =
        Arc::new(SoulReviewEngine::load(&config_dir).context("SoulReviewEngine 初始化失败")?);

    let review_config = engine.config().review.clone();

    // 创建治理轮询任务的关闭信号通道
    let (poll_shutdown_tx, poll_shutdown_rx) = tokio::sync::watch::channel(false);

    // 启动周期审议任务（持有 JoinHandle 以便优雅关闭）
    let engine_clone = engine.clone();
    let store_clone = proposal_store.clone();
    let cm_clone = connection_manager.clone();
    let gdc_clone = game_data_cache.clone();
    let poll_interval = review_config.poll_interval_secs;
    let governance_poll_handle = tokio::spawn(async move {
        let mut shutdown_rx = poll_shutdown_rx;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(poll_interval));
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("治理轮询任务收到关闭信号，退出循环");
                        break;
                    }
                }
                _ = interval.tick() => {
                    // 单次轮询 batch 总时间预算：review_config.timeout_secs
                    // 默认 1800s，可容纳多 group × 多 LLM 调用 × 单次 LLM request_timeout
                    let review_timeout = std::time::Duration::from_secs(review_config.timeout_secs);

                    // 超时清理：仅关闭 awaiting_fuxi_initial 阶段超时的 group
                    // 已进入 awaiting_peer / awaiting_fuxi_final 的 group 不关闭（管道会重试）
                    let stale_secs = review_config.group_stale_secs;
                    if let Ok(closed) = store_clone.close_stale_groups(stale_secs).await
                        && closed > 0
                    {
                        info!("治理轮询: 强制关闭 {} 个超时 group（awaiting_fuxi_initial）", closed);
                    }

                    let pending_result =
                        tokio::time::timeout(review_timeout, store_clone.get_pending_groups()).await;
                    match pending_result {
                        Ok(Ok(groups)) if !groups.is_empty() => {
                            let review_future = engine_clone.review_pending(&store_clone, &groups);
                            let review_result =
                                tokio::time::timeout(review_timeout, review_future).await;
                            match review_result {
                                Ok(results) => {
                                    for (group_id, status) in &results {
                                        info!("Group {} 审议完成: {}", group_id, status);
                                        if *status == cyber_jianghu_server::governance::ProposalStatus::Approved {
                                            // Auto-evolve 已在 engine.review_group() 中写入 actions.yaml
                                            // 重新加载 ActionRegistry 到内存
                                            match cyber_jianghu_server::game_data::loaders::load_actions(
                                                cyber_jianghu_server::paths::get_config_dir(),
                                            ) {
                                                Ok(new_actions) => {
                                                    gdc_clone.update_actions(new_actions);
                                                    info!("ActionRegistry 已更新（auto-evolution）");
                                                }
                                                Err(e) => {
                                                    warn!("ActionRegistry 重载失败: {}", e);
                                                }
                                            }

                                            // 刷新 CapabilityManifest（使 LLM 下轮审议看到新 action）
                                            engine_clone.reload_manifest().await;

                                            let actions_path = cyber_jianghu_server::paths::get_config_dir().join("actions.yaml");
                                            let actions_content = match std::fs::read_to_string(&actions_path) {
                                                Ok(c) => c,
                                                Err(e) => {
                                                    error!(
                                                        "Approved group {}: 读取 actions.yaml 失败，跳过广播避免破坏 agent 端缓存: {}",
                                                        group_id, e
                                                    );
                                                    continue;
                                                }
                                            };
                                            let config_update = cyber_jianghu_protocol::messages::ServerMessage::config_update_full_value(
                                                cyber_jianghu_protocol::ConfigType::Actions,
                                                chrono::Utc::now().to_rfc3339(),
                                                serde_json::json!({"yaml": actions_content}),
                                                None,
                                            );
                                            if let Err(e) = cyber_jianghu_server::websocket::broadcast_config_update(config_update, &cm_clone).await {
                                                warn!("Approved group {} broadcast 失败: {}", group_id, e);
                                            }
                                        }
                                    }
                                }
                                Err(_) => {
                                    warn!(
                                        "Group 批次审议超时（>{}s），跳过本轮",
                                        review_timeout.as_secs()
                                    );
                                }
                            }
                        }
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            warn!("获取待审议 groups 失败: {}", e);
                        }
                        Err(_) => {
                            warn!(
                                "获取待审议 groups 超时（>{}s），跳过本轮",
                                review_timeout.as_secs()
                            );
                        }
                    }
                }
            }
        }
        info!("治理轮询任务已停止");
    });

    let state = GovernanceState {
        classifier,
        proposal_store,
        engine,
        connection_manager,
        review_config,
    };

    let shutdown = GovernanceShutdown {
        shutdown_tx: poll_shutdown_tx,
        handle: governance_poll_handle,
    };

    Ok((state, shutdown))
}

// ============================================================================
// 主函数
// ============================================================================
