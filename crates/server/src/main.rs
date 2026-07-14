// ============================================================================
// OpenClaw Cyber-Jianghu MVP 服务端主入口
// ============================================================================
//
// 这是整个服务端的入口点，负责：
// 1. 初始化日志和配置
// 2. 启动Tick引擎（后台任务）
// 3. 启动Web服务器（HTTP + WebSocket）
//
// 架构说明：
// - Tick引擎在独立的tokio任务中运行，负责驱动游戏世界
// - Web服务器在主任务中运行，处理HTTP请求和WebSocket连接
// - 两者通过Arc<AppState>共享配置和状态
//
// MVP阶段功能：
// - 基础的HTTP API（健康检查、Agent注册）
// - Tick引擎框架（待完善）
// - WebSocket框架（待实现）
// ============================================================================

// 引入 library crate
use cyber_jianghu_server::governance::{
    ActionEvolutionConfig, ProposalStore, SoulReviewEngine, TopicClassifier,
};
use cyber_jianghu_server::state::{
    GovernanceState, create_agent_state_cache, populate_agent_state_cache,
};
use cyber_jianghu_server::tick::{IntentWorker, StateProcessor, create_worker_channel};
use cyber_jianghu_server::*;

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Body,
    routing::{delete, get, post},
};
use std::fs::OpenOptions;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

// ============================================================================
// Tick引擎启动
// ============================================================================

fn serve_admin_file(path: &str) -> Result<axum::response::Response<Body>, StatusCode> {
    let static_dir = match std::fs::canonicalize(crate::paths::get_static_dir().join("admin")) {
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
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, mime.as_ref())
        .body(body)
        .unwrap_or_else(|_| axum::response::Response::new(Body::empty())))
}

fn render_admin_token_file_content(
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
fn ensure_admin_token_permissions(path: &Path) -> Result<()> {
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
fn ensure_admin_token_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn write_admin_token_file(
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
            .with_context(|| format!("无法创建admin token文件 {}: {}", path.display(), "open failed"))?
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
async fn serve_admin_index() -> Result<axum::response::Response<Body>, StatusCode> {
    serve_admin_file("index.html")
}

/// /admin/{*path} → serve the specific file
async fn serve_admin(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> Result<axum::response::Response<Body>, StatusCode> {
    serve_admin_file(&path)
}

use axum::http::StatusCode;

/// 启动Tick引擎（后台任务）
///
/// Tick引擎在独立的tokio任务中运行，负责驱动游戏世界
#[allow(clippy::too_many_arguments)]
fn start_tick_engine(
    game_data_cache: Arc<game_data::GameDataCache>,
    db_pool: DbPool,
    connection_manager: websocket::ConnectionManager,
    agent_to_device_map: websocket::AgentToDeviceMap,
    worker_tx: tokio::sync::mpsc::Sender<cyber_jianghu_server::tick::WorkerMessage>,
    agent_state_cache: cyber_jianghu_server::state::AgentStateCache,
    accepting_tick_id: Arc<AtomicI64>,
    vendor_pending_events: cyber_jianghu_server::models::VendorPendingEvents,
    prompt_template_cache: Arc<tokio::sync::RwLock<Option<crate::state::PromptTemplateCache>>>,
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
struct GovernanceShutdown {
    /// 关闭信号发送端：调用 `send(true)` 通知任务退出
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// 任务 JoinHandle：在 shutdown 时 await，等待任务收尾
    handle: JoinHandle<()>,
}

/// 初始化治理系统
///
/// 加载 action_evolution.yaml 配置，创建 TopicClassifier、ProposalStore、SoulReviewEngine，
/// 并启动周期审议后台任务。返回 `(GovernanceState, GovernanceShutdown)`，
/// 调用方负责在关闭时通过 `GovernanceShutdown` 优雅停掉轮询任务。
async fn init_governance(
    db_pool: &DbPool,
    connection_manager: websocket::ConnectionManager,
    game_data_cache: Arc<game_data::GameDataCache>,
) -> Result<(GovernanceState, GovernanceShutdown)> {
    let config_dir = crate::paths::get_config_dir();

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
                                        if *status == crate::governance::ProposalStatus::Approved {
                                            // Auto-evolve 已在 engine.review_group() 中写入 actions.yaml
                                            // 重新加载 ActionRegistry 到内存
                                            match crate::game_data::loaders::load_actions(
                                                crate::paths::get_config_dir(),
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

                                            let actions_path = crate::paths::get_config_dir().join("actions.yaml");
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
                                            let config_update = cyber_jianghu_protocol::messages::ServerMessage::ConfigUpdate {
                                                config_type: "actions".to_string(),
                                                update_type: "full".to_string(),
                                                version: chrono::Utc::now().to_rfc3339(),
                                                content: serde_json::json!({"yaml": actions_content}),
                                                content_hash: None,
                                                updated_items: vec![],
                                                removed_items: vec![],
                                            };
                                            if let Err(e) = crate::websocket::broadcast_config_update(config_update, &cm_clone).await {
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

#[tokio::main]
#[allow(clippy::await_holding_lock)]
async fn main() -> Result<()> {
    // 1. 先加载 .env，确保 RUST_LOG 等环境变量对日志 subscriber 生效（P1-F2 修复）
    let _ = dotenv::dotenv();

    // 2. 初始化日志（P1-F2 修复：EnvFilter::try_from_default_env 消费 RUST_LOG，替代硬编码 Level::INFO）
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    // JSON 切换：CYBER_JIANGHU_LOG_JSON=1 → .json()；默认 .compact() 输出人可读
    let log_json = std::env::var("CYBER_JIANGHU_LOG_JSON")
        .ok()
        .as_deref()
        == Some("1");
    let fmt_builder = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_thread_ids(false);
    if log_json {
        fmt_builder.json().init();
    } else {
        fmt_builder.init();
    }

    info!(
        "OpenClaw Cyber-Jianghu MVP Server v{}",
        env!("CARGO_PKG_VERSION")
    );
    info!("天道无为，万物自化。");

    // 打印关键路径信息
    info!("运行时路径配置:");
    info!("  Config: {:?}", crate::paths::get_config_dir());
    info!("  Static: {:?}", crate::paths::get_static_dir());
    info!("  Logs:   {:?}", crate::paths::get_logs_dir());

    // 2. 加载环境变量（从.env文件）
    dotenv::dotenv().ok();

    // 3. 加载配置
    let config = Config::load()?;
    info!("配置加载成功");

    // 4. 验证配置
    config.validate()?;
    info!("配置验证通过");

    // 5. 初始化数据库连接池
    let db_pool = init_db_pool(&config.database).await?;
    info!("数据库连接池初始化成功");
    let db_runtime_health = crate::db::create_db_runtime_health_state();
    let _db_probe_handle = crate::db::start_db_health_probe(
        db_pool.clone(),
        db_runtime_health.clone(),
        std::time::Duration::from_secs(config.database.probe_interval_secs),
    );

    // 6. 加载游戏数据配置
    let game_data = game_data::load_game_data()?;
    info!(
        "游戏数据配置加载成功 (version: {})",
        game_data.game_rules.version
    );

    // 创建游戏数据缓存并初始化统一注册表
    let game_data_cache = Arc::new(game_data::GameDataCache::new(game_data));
    game_data::init_registry(game_data_cache.clone());
    info!("统一配置注册表初始化完成");

    // P0-1: 配置完整性校验（warning 模式，不阻断启动）
    match crate::config_validator::load_rules() {
        Ok(rules) => {
            let result = crate::config_validator::run_all_validations(&rules);
            if !result.violations.is_empty() {
                warn!("配置完整性检查发现 {} 条违规:", result.violations.len());
                for v in &result.violations {
                    warn!(
                        "  [规则 {}] {}: {} → {}: {}",
                        v.rule_index, v.source_type, v.source_value, v.target_type, v.message
                    );
                }
            }
            info!(
                "配置完整性检查完成: {} passed, {} failed",
                result.passed, result.failed
            );
        }
        Err(e) => {
            warn!("加载 validation_rules.yaml 失败，跳过配置完整性检查: {}", e);
        }
    }

    // 初始化物品系统缓存（物品需要独立的缓存用于快速查询）
    {
        let guard = game_data_cache.get();
        items::init_item_cache_from_config(&guard.items.data)?;
        info!("物品系统初始化完成，共 {} 种物品", guard.items.data.len());

        // 同步物品到数据库（用于外键约束）
        if let Err(e) = db::sync_items_from_config(&db_pool, &guard.items.data).await {
            error!("同步物品到数据库失败: {}", e);
        }
    }

    // 7. 初始化 WebSocket 连接管理器、agent→device 映射器和速率限制器
    let connection_manager = websocket::create_connection_manager();
    let agent_to_device_map = websocket::create_agent_to_device_map();
    let rate_limiter = create_rate_limiter();
    let device_register_limiter = cyber_jianghu_server::state::create_device_register_limiter();
    info!("WebSocket 和速率限制器初始化成功");

    // 7.2 初始化 Agent 状态内存缓存（从 DB 加载）
    let agent_state_cache = create_agent_state_cache();
    let cached_count = populate_agent_state_cache(&agent_state_cache, &db_pool).await?;
    info!("Agent 状态缓存初始化完成，加载 {} 个 Agent", cached_count);

    // 7.1 初始化对话管理器（从配置读取最大消息数）
    let gd_guard = game_data_cache.get();
    let dialogue_manager = Arc::new(dialogue::DialogueManager::new(
        gd_guard.network.data.dialogue.max_messages_per_agent,
    ));
    drop(gd_guard); // 释放锁
    info!("对话管理器初始化成功");

    // 7.3 创建 IntentWorker channel 并启动 Worker
    let (worker_tx, worker_rx) = create_worker_channel();
    let state_processor = Arc::new(StateProcessor::new(db_pool.clone()));
    let intent_worker = IntentWorker::new(
        db_pool.clone(),
        agent_state_cache.clone(),
        state_processor,
        connection_manager.clone(),
        agent_to_device_map.clone(),
        dialogue_manager.clone(),
        game_data_cache.clone(),
    );
    tokio::spawn(async move {
        intent_worker.run(worker_rx).await;
    });
    info!("IntentWorker 启动");

    // 8. 获取或生成管理 Token
    let admin_read_token = config
        .server
        .admin_read_token
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let admin_write_token = config
        .server
        .admin_write_token
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let read_token_source = if config.server.admin_read_token.is_some() {
        "配置/环境变量"
    } else {
        "自动生成"
    };
    let write_token_source = if config.server.admin_write_token.is_some() {
        "配置/环境变量"
    } else {
        "自动生成"
    };

    let token_path = crate::paths::get_logs_dir().join("cyber_jianghu_admin.tmp");
    write_admin_token_file(
        &token_path,
        read_token_source,
        &admin_read_token,
        write_token_source,
        &admin_write_token,
    )?;

    info!("管理员访问凭证已保存到: {}", token_path.display());
    info!("查看凭证: cat {}", token_path.display());

    // 9. 创建共享 tick_id（scheduler 和 AppState 共用）
    let accepting_tick_id = Arc::new(AtomicI64::new(0));

    // 9.1 加载/初始化服务器部署时间（持久化，重启不变）
    let deployment_time = crate::db::get_or_init_deployment_time(&db_pool)
        .await
        .context("加载服务器部署时间失败")?;
    info!(
        "服务器部署时间: {} (已运行 {})",
        deployment_time,
        chrono::Utc::now().signed_duration_since(deployment_time)
    );

    // 9.2 初始化治理系统
    let (governance, mut governance_shutdown) = match init_governance(
        &db_pool,
        connection_manager.clone(),
        game_data_cache.clone(),
    )
    .await
    {
        Ok((g, s)) => {
            info!("治理系统初始化成功");
            (Some(g), Some(s))
        }
        Err(e) => {
            warn!("治理系统初始化失败（将继续运行，治理功能不可用）: {}", e);
            (None, None)
        }
    };

    // 9.3 创建应用状态
    let state = Arc::new(AppState::new(
        db_pool.clone(),
        db_runtime_health,
        connection_manager.clone(),
        agent_to_device_map.clone(),
        agent_state_cache.clone(),
        worker_tx.clone(),
        rate_limiter.clone(),
        device_register_limiter,
        game_data_cache.clone(),
        dialogue_manager.clone(),
        admin_read_token,
        admin_write_token,
        deployment_time,
        crate::paths::get_config_dir(),
        accepting_tick_id.clone(),
        governance,
    ));

    // 10. 启动Tick引擎（后台任务）
    let tick_engine_handle = start_tick_engine(
        game_data_cache.clone(),
        db_pool.clone(),
        connection_manager.clone(),
        agent_to_device_map.clone(),
        worker_tx.clone(),
        agent_state_cache.clone(),
        accepting_tick_id,
        state.vendor_pending_events.clone(),
        state.prompt_template_cache.clone(),
    );

    // 10.1 启动速率限制器清理任务
    let _cleanup_handle = start_rate_limiter_cleanup(rate_limiter.clone());

    // 10.2 启动遥测采集器（后台定时任务，不阻塞主流程）
    let telemetry_handles = telemetry::start_telemetry_collector(db_pool.clone());

    // 11. 构建路由
    let app = Router::new()
        .route("/", get(handlers::system::root))
        .route("/health", get(handlers::system::health_check))
        // 设备身份生命周期 v2 — 严格校验（DB 不存在时返回 404）
        .route(
            "/api/v1/device/verify",
            post(handlers::device::device_verify),
        )
        // 设备身份生命周期 v2 — 显式注册（server 生成 device_id，201 Created）
        .route(
            "/api/v1/device/register",
            post(handlers::device::device_register),
        )
        // 角色注册（Phase 4）- 创建游戏角色
        .route(
            "/api/v1/agent/register",
            post(handlers::agent::agent_register),
        )
        // 角色归隐 - 将活跃角色标记为 retired，允许创建新角色
        .route("/api/v1/agent/retire", post(handlers::agent::agent_retire))
        // 自动重生 - Agent 死亡后延迟调用
        .route(
            "/api/v1/agent/auto-rebirth",
            post(handlers::agent::agent_auto_rebirth),
        )
        // 管理员库存注入（Vendor 补货等）
        .route(
            "/api/v1/agent/grant-items",
            post(handlers::agent::agent_grant_items).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_write_token,
            )),
        )
        // 传记回传 - Agent 端死亡/归隐时将纪传体传记回传 server
        .route(
            "/api/v1/agent/biography",
            post(handlers::agent::update_biography),
        )
        // 传记查询 - Agent 端回退读取（本地无传记时从 server DB 获取）
        .route(
            "/api/v1/agent/{id}/biography",
            get(handlers::agent::get_agent_biography),
        )
        // Prompt Templates 拉取 — Agent 启动时主动获取
        .route(
            "/api/v1/agent/prompt-templates",
            post(handlers::agent::get_prompt_templates),
        )
        // Vendor 补货规则管理
        .route(
            "/api/dashboard/agent/{id}/vendor-refill",
            get(handlers::vendor::get_vendor_refill_rules)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ))
                .put(handlers::vendor::set_vendor_refill_rule)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                )),
        )
        .route(
            "/api/dashboard/agent/{id}/vendor-refill/{item_id}",
            delete(handlers::vendor::delete_vendor_refill_rule).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agent/{id}/roles",
            get(handlers::role::get_agent_roles_handler)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ))
                .post(handlers::role::assign_role_handler)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                )),
        )
        .route(
            "/api/dashboard/roles",
            get(handlers::role::list_available_roles).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        .route(
            "/api/dashboard/agent/{id}/roles/{role_key}",
            delete(handlers::role::remove_role_handler).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        .route(
            "/api/v1/agent/{id}/context",
            get(handlers::context::get_agent_context),
        )
        .route(
            "/api/v1/validate-action",
            post(handlers::validation::validate_action),
        )
        .route("/ws", get(websocket::websocket_handler))
        // Dashboard API - 无需认证
        .route(
            "/api/dashboard/actions-map",
            get(handlers::dashboard::get_actions_map),
        )
        .route(
            "/api/dashboard/items",
            get(handlers::dashboard::get_items).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        // 展示名映射（经历日志前端翻译 agent_id/item_id 用）
        .route(
            "/api/dashboard/display-map",
            get(handlers::dashboard::get_display_map).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        // 天魂层展示名映射（数据驱动，从 souls.yaml 读取）
        .route(
            "/api/dashboard/layer-display",
            get(handlers::dashboard::get_layer_display),
        )
        // Dashboard API (需要 Read 权限)
        .route(
            "/api/dashboard/stats",
            get(handlers::dashboard::get_dashboard_stats).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/reward/trends",
            get(handlers::dashboard::get_reward_trends).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/reward/lifetime/{id}",
            get(handlers::dashboard::get_agent_lifetime_reward).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/emergence",
            get(handlers::dashboard::get_emergence).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/health",
            get(handlers::dashboard::get_health).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agents/offline",
            get(handlers::dashboard::get_offline_agents).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agents/dead",
            get(handlers::dashboard::get_dead_agents).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        .route(
            "/api/dashboard/agent/{id}",
            get(handlers::dashboard::get_agent_details).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agent/{id}/experiences",
            get(handlers::dashboard::get_agent_experiences).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agents",
            get(handlers::dashboard::get_all_agents).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        .route(
            "/api/dashboard/status-configs",
            get(handlers::dashboard::get_status_configs).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/experiences",
            get(handlers::dashboard::get_experiences).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        .route(
            "/api/dashboard/agents/cleanup",
            post(handlers::dashboard::cleanup_offline_agents).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        // Chronicle API (群像传记)
        .route(
            "/api/dashboard/chronicles",
            get(handlers::chronicle::list_chronicles).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        .route(
            "/api/dashboard/chronicles/{id}",
            get(handlers::chronicle::get_chronicle).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        .route(
            "/api/dashboard/chronicles/generate",
            post(handlers::chronicle::generate_chronicle).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        // LLM Token 统计
        .route(
            "/api/dashboard/chronicles/llm-stats",
            get(handlers::chronicle::get_llm_stats).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        // 异步生成任务进度
        .route(
            "/api/dashboard/chronicles/pending",
            get(handlers::chronicle::get_pending_generations).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        // Agent 每日摘要 API
        .route(
            "/api/dashboard/agent-daily-summaries",
            get(handlers::agent_daily_summaries::list_summaries).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/agent-daily-summaries/{agent_id}",
            get(handlers::agent_daily_summaries::get_by_agent).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        // Config API (List/Get 需要 Read 权限, Update 需要 Write 权限)
        .route(
            "/api/config",
            get(handlers::config_editor::list_configs).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        .route(
            "/api/config/{filename}",
            get(handlers::config_editor::get_config_content)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ))
                .put(handlers::config_editor::update_config_content)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                )),
        )
        // LLM Config API (独立于通用配置编辑器)
        // 前端 settings.html 经 API.BASE="/api/dashboard" 发请求，
        // 故路由需带 /dashboard 前缀以与其它 dashboard 接口一致。
        .route(
            "/api/dashboard/config/llm",
            get(handlers::config_llm::get_llm_config)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ))
                .post(handlers::config_llm::save_llm_config)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                )),
        )
        // LLM Status & Enabled API
        .route(
            "/api/dashboard/config/llm/status",
            get(handlers::config_llm::get_llm_status).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        .route(
            "/api/dashboard/config/llm/enabled",
            get(handlers::config_llm::get_llm_enabled)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ))
                .post(handlers::config_llm::set_llm_enabled)
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                )),
        )
        // Config Reload API (需要 Write 权限)
        .route(
            "/api/admin/reload-config",
            post(handlers::config_reload::reload_config_handler).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        // Admin Auth API (Cookie Session)
        .route("/api/admin/login", post(handlers::admin_auth::login))
        .route("/api/admin/logout", post(handlers::admin_auth::logout))
        .route(
            "/api/admin/session",
            get(handlers::admin_auth::check_session),
        )
        // Admin Static Files (no auth - login page must be accessible without token)
        // Auth is enforced client-side: frontend stores token in localStorage,
        // sends it via Bearer header on API calls. API routes have their own middleware.
        .route("/admin/", get(serve_admin_index))
        .route("/admin/{*path}", get(serve_admin))
        // Redirect /admin to /admin/
        .route(
            "/admin",
            get(|| async { axum::response::Redirect::temporary("/admin/") }),
        )
        // Action Evolution — 管理面板统计
        .route(
            "/api/dashboard/action-evolution/stats",
            get(handlers::dashboard::get_action_evolution_stats).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        // Action Evolution — 提案组列表（支持 status 过滤）
        .route(
            "/api/dashboard/action-evolution/groups",
            get(handlers::dashboard::get_proposal_groups).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        // Action Evolution — 提案组详情
        .route(
            "/api/dashboard/action-evolution/groups/{id}",
            get(handlers::dashboard::get_proposal_group_detail).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        // Action Evolution — 管理员审批/驳回提案组
        .route(
            "/api/dashboard/action-evolution/groups/{id}/action",
            post(handlers::dashboard::admin_action_on_group).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_write_token,
                ),
            ),
        )
        // Telemetry API — 行为遥测聚合查询
        .route(
            "/api/dashboard/telemetry",
            get(handlers::dashboard::list_telemetry_aggregations).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        .route(
            "/api/dashboard/telemetry/{aggregation_name}",
            get(handlers::dashboard::get_telemetry_aggregation).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_read_token,
                ),
            ),
        )
        // Action Evolution — 治理提案提交（Agent 设备认证）
        .route(
            "/api/v1/action-evolution/propose",
            post(crate::governance::handlers::submit_proposal).layer(
                axum::middleware::from_fn_with_state(
                    state.clone(),
                    handlers::auth::require_device_token,
                ),
            ),
        )
        .with_state(state);

    // 12. 启动Web服务器
    let addr = SocketAddr::new(config.server.host.parse()?, config.server.port);

    // Get tick duration from game_data for logging
    let tick_duration_secs = {
        let gd = game_data_cache.get();
        gd.game_rules.data.agent_state.tick.real_seconds_per_tick
    };

    info!("启动服务器于 {}", addr);
    info!("注意：生产环境请务必通过 Nginx/Traefik 启用 WSS (WebSocket Secure)");
    info!("健康检查: http://{}/health", addr);
    info!("Agent注册: POST http://{}/api/v1/agent/register", addr);
    info!("Tick周期: {}秒 (来自 game_rules.yaml)", tick_duration_secs);
    info!("服务启动完成，等待连接...");
    info!(
        "WebSocket端点: ws://{}:{}/ws?token=YOUR_AUTH_TOKEN",
        addr.ip(),
        addr.port()
    );

    // 12. 启动监听
    let listener = tokio::net::TcpListener::bind(addr).await?;

    // 13. 注册信号处理（优雅关闭）
    // P1-9 修复：用单一 watch::channel 把 shutdown 信号广播给 axum::serve 与主 select!。
    // 之前 shutdown_signal 是只被一个分支消费的 async block，axum::serve 没挂上
    // .with_graceful_shutdown(...) → SIGTERM 触发时 in-flight HTTP 请求被截断。
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    // 给 axum 用的 receiver（独立 clone，避免主 select! 抢先消费）
    let axum_shutdown_rx = shutdown_rx.clone();
    let axum_shutdown = async move {
        let mut rx = axum_shutdown_rx;
        let _ = rx.changed().await;
    };
    // 给主 select! 用的 receiver
    let mut main_shutdown_rx = shutdown_rx;

    // 后台任务：监听 SIGINT/SIGTERM，触发时给 watch channel 发送 true，
    // 唤醒 axum 的 graceful_shutdown 与主 select! 的 shutdown 分支。
    tokio::spawn(async move {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };
        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install signal handler")
                .recv()
                .await;
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();
        tokio::select! {
            _ = ctrl_c => info!("收到 SIGINT 信号 (Ctrl+C)"),
            _ = terminate => info!("收到 SIGTERM 信号"),
        }
        if let Err(e) = shutdown_tx.send(true) {
            tracing::warn!("shutdown_tx.send 失败（receiver 可能已 drop）：{e:?}");
        }
    });

    // 14. 等待服务器结束、Tick引擎失败、治理轮询失败或关闭信号
    tokio::select! {
        // 关闭信号（P1-9：与 axum::serve.with_graceful_shutdown 共用 watch channel）
        _ = async {
            let _ = main_shutdown_rx.changed().await;
        } => {
            info!("正在关闭服务...");
            info!("服务已优雅关闭");
        }

        // Web服务器运行（P1-9：挂上 .with_graceful_shutdown 让 in-flight 请求 drain）
        result = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(axum_shutdown) => {
            if let Err(e) = result {
                error!("Web服务器错误: {}", e);
            }
        }

        // Tick引擎任务
        result = tick_engine_handle => {
            if let Err(e) = result {
                error!("Tick引擎任务失败: {}", e);
            }
        }

        // 遥测采集器（意外退出也算失败路径）
        _ = async {
            for h in telemetry_handles {
                let _ = h.await;
            }
        } => {
            info!("遥测采集器已退出");
        }

        // 治理轮询任务（意外退出也算失败路径）
        result = async {
            match governance_shutdown.as_mut() {
                Some(s) => (&mut s.handle).await,
                None => std::future::pending::<Result<(), tokio::task::JoinError>>().await,
            }
        } => {
            match result {
                Ok(()) => warn!("治理轮询任务已退出（正常）"),
                Err(e) => error!("治理轮询任务失败: {}", e),
            }
        }
    }

    // 15. 触发治理轮询关闭信号并等待任务收尾（带超时）
    if let Some(shutdown) = governance_shutdown {
        let _ = shutdown.shutdown_tx.send(true);
        match tokio::time::timeout(std::time::Duration::from_secs(5), shutdown.handle).await {
            Ok(Ok(())) => info!("治理轮询任务已优雅退出"),
            Ok(Err(e)) => error!("治理轮询任务 join 失败: {}", e),
            Err(_) => warn!("治理轮询任务未在 5s 内退出，继续主流程"),
        }
    }

    info!("服务停止");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{render_admin_token_file_content, write_admin_token_file};

    #[test]
    fn test_render_admin_token_file_content_is_ascii_without_emoji_header() {
        let content = render_admin_token_file_content("自动生成", "read-token", "配置", "write-token");
        assert!(!content.contains("🔐"));
        assert!(content.contains("Cyber-Jianghu 管理员访问凭证"));
    }

    /// 验证 P1-9：axum::serve 必须链式调用 `.with_graceful_shutdown(...)`，
    /// 否则 SIGTERM/SIGINT 触发时 in-flight HTTP 请求会被截断，
    /// DB 写入半完成、Saga 状态不一致（高风险）。
    #[test]
    fn test_p1_9_axum_serve_uses_graceful_shutdown() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = std::fs::read_to_string(manifest_dir.join("src/main.rs"))
            .expect("read main.rs source");
        // 仅扫描生产代码段（`mod tests` 之前），避免测试自身字符串假阳性
        let tests_marker = source
            .find("#[cfg(test)]")
            .expect("main.rs should have a #[cfg(test)] block");
        let prod_slice = &source[..tests_marker];
        let serve_idx = prod_slice
            .find("axum::serve(")
            .expect("must call axum::serve in production code");
        let tail = &prod_slice[serve_idx..];
        let next_400 = tail.get(..400).unwrap_or(tail);
        assert!(
            next_400.contains(".with_graceful_shutdown("),
            "P1-9 修复缺失：axum::serve 必须链式调用 .with_graceful_shutdown(...)，\n\
             否则 SIGTERM/SIGINT 触发时 in-flight HTTP 请求会被截断，\n\
             DB 写入半完成、Saga 状态不一致。\n\
             当前 axum::serve 后续 400 字符片段：\n{next_400}"
        );
    }

    /// 验证 P1-F2：tracing subscriber 必须消费 `RUST_LOG` 环境变量，
    /// 而不是硬编码 `Level::INFO`。同时 `.env` 加载必须在 subscriber init 之前，
    /// 否则 `.env` 里的 `RUST_LOG=debug` 不会生效。
    #[test]
    fn test_p1_f2_tracing_uses_env_filter_and_dotenv_first() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = std::fs::read_to_string(manifest_dir.join("src/main.rs"))
            .expect("read main.rs source");
        let tests_marker = source
            .find("#[cfg(test)]")
            .expect("main.rs should have a #[cfg(test)] block");
        let prod = &source[..tests_marker];

        // 1. 必须使用 EnvFilter::try_from_default_env
        assert!(
            prod.contains("EnvFilter::try_from_default_env"),
            "P1-F2 修复缺失：tracing subscriber 必须消费 RUST_LOG；\
             用 `EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(\"info\"))` 替代硬编码 `Level::INFO`"
        );
        // 2. 不应在生产路径上继续用 FmtSubscriber::builder().with_max_level(Level::INFO) 硬编码
        assert!(
            !prod.contains(".with_max_level(Level::INFO)"),
            "P1-F2 修复：禁止继续用 `.with_max_level(Level::INFO)` 硬编码日志级别"
        );

        // 3. dotenv 必须在 tracing subscriber 初始化之前
        //    新代码用 `.init()`，旧代码用 `set_global_default`，都接受。
        let dotenv_idx = prod
            .find("dotenv::dotenv()")
            .expect("must call dotenv::dotenv()");
        let init_idx = prod
            .find("tracing_subscriber::fmt()")
            .or_else(|| prod.find("FmtSubscriber::builder"))
            .or_else(|| prod.find("set_global_default"))
            .expect("must initialize tracing subscriber");
        assert!(
            dotenv_idx < init_idx,
            "P1-F2 修复：dotenv::dotenv() 必须在 tracing subscriber 初始化之前调用，\
             否则 .env 里的 RUST_LOG 不会生效。\
             dotenv_idx={dotenv_idx}, init_idx={init_idx}"
        );
    }

    /// 验证 P1-F2：workspace Cargo.toml 的 tracing-subscriber 必须启用 `json` feature，
    /// 否则代码里写 `.json()` 编译失败。
    #[test]
    fn test_p1_f2_workspace_tracing_subscriber_enables_json_feature() {
        // workspace root is ../../ from crates/server
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace_toml = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .expect("crates/server should be inside a workspace")
            .join("Cargo.toml");
        let content = std::fs::read_to_string(&workspace_toml)
            .expect("read workspace Cargo.toml");
        // 找 tracing-subscriber = {...} 这行
        let ts_line = content
            .lines()
            .find(|l| l.contains("tracing-subscriber") && l.contains("="))
            .expect("workspace must declare tracing-subscriber");
        assert!(
            ts_line.contains("\"json\""),
            "P1-F2 修复：workspace Cargo.toml 的 tracing-subscriber 必须启用 \"json\" feature（用于 JSON 化日志），当前行：\n{ts_line}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_write_admin_token_file_enforces_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("create temp dir");
        let file_path = dir.path().join("admin_tokens.txt");

        write_admin_token_file(&file_path, "自动生成", "read-token", "配置", "write-token")
            .expect("write admin token file");

        let mode = std::fs::metadata(&file_path)
            .expect("read metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
