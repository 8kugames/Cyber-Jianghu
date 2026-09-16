//! server main 流程（自 main.rs 外移的 main 函数体）

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
use cyber_jianghu_server::tick::{IntentWorker, StateProcessor, create_worker_channel};
use cyber_jianghu_server::*;

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const TRAINING_EXPORT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

// ============================================================================
// Tick引擎启动
// ============================================================================

use super::admin_static::{init_governance, start_tick_engine, write_admin_token_file};
use super::router::build_router;
use cyber_jianghu_server::state::{create_agent_state_cache, populate_agent_state_cache};

#[allow(clippy::await_holding_lock)]
pub(crate) async fn run() -> Result<()> {
    // 1. 先加载 .env，确保 RUST_LOG 等环境变量对日志 subscriber 生效
    let _ = dotenv::dotenv();

    // 2. 初始化日志（EnvFilter::try_from_default_env 消费 RUST_LOG，替代硬编码 Level::INFO）
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // JSON 切换：CYBER_JIANGHU_LOG_JSON=1 → .json()；默认 .compact() 输出人可读
    let log_json = std::env::var("CYBER_JIANGHU_LOG_JSON").ok().as_deref() == Some("1");
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
    info!(
        "  Config: {:?}",
        cyber_jianghu_server::paths::get_config_dir()
    );
    info!(
        "  Static: {:?}",
        cyber_jianghu_server::paths::get_static_dir()
    );
    info!(
        "  Logs:   {:?}",
        cyber_jianghu_server::paths::get_logs_dir()
    );

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

    // C0: 非 docker 部署时自动跑迁移（docker 由 entrypoint 处理，幂等 SQL 可安全重复）
    cyber_jianghu_server::db::run_migrations(&db_pool).await?;
    let db_runtime_health = cyber_jianghu_server::db::create_db_runtime_health_state();
    let _db_probe_handle = cyber_jianghu_server::db::start_db_health_probe(
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

    // 配置完整性校验（warning 模式，不阻断启动）
    match cyber_jianghu_server::config_validator::load_rules() {
        Ok(rules) => {
            let result = cyber_jianghu_server::config_validator::run_all_validations(&rules);
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

    // 游戏客户端只读 Token：不自动生成。
    // None 表示禁用客户端鉴权档，require_client_read_token 回退到 admin read token。
    let client_read_token = config.server.client_read_token.clone();
    if client_read_token.is_some() {
        info!("客户端只读档已启用 (CLIENT_READ_TOKEN)，前端可使用低权限 token 取数据");
    } else {
        info!("客户端只读档未配置，require_client_read_token 将回退到 admin read token");
    }

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

    let token_path = cyber_jianghu_server::paths::get_logs_dir().join("cyber_jianghu_admin.tmp");
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
    let deployment_time = cyber_jianghu_server::db::get_or_init_deployment_time(&db_pool)
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

    // 9.3 加载并按开关启动训练导出 scheduler。
    let training_export_config = cyber_jianghu_server::training_export::config::load_config(
        &cyber_jianghu_server::paths::get_config_dir(),
    )
    .context("加载 training_export 配置失败")?;
    info!(
        enabled = training_export_config.enabled,
        interval_secs = training_export_config.scheduler.interval_secs,
        "training_export 配置加载完成"
    );
    let (manual_tx, training_exporter_shutdown) = if training_export_config.enabled {
        let (manual_tx, shutdown) =
            cyber_jianghu_server::training_export::scheduler::start_training_exporter(
                training_export_config.clone(),
                db_pool.clone(),
            );
        (Some(manual_tx), Some(shutdown))
    } else {
        info!("training_export 未启用, 不启动后台 task");
        (None, None)
    };
    let training_export_handle =
        cyber_jianghu_server::training_export::handlers::TrainingExportHandle {
            config: training_export_config,
            manual_tx,
        };

    // 9.4 创建应用状态
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
        client_read_token,
        deployment_time,
        cyber_jianghu_server::paths::get_config_dir(),
        accepting_tick_id.clone(),
        governance,
        training_export_handle,
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

    let app = build_router(state.clone());

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
    // 用单一 watch::channel 把 shutdown 信号广播给 axum::serve 与主 select!。
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
        // 关闭信号（与 axum::serve.with_graceful_shutdown 共用 watch channel）
        _ = async {
            let _ = main_shutdown_rx.changed().await;
        } => {
            info!("正在关闭服务...");
            info!("服务已优雅关闭");
        }

        // Web服务器运行（挂上 .with_graceful_shutdown 让 in-flight 请求 drain）
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

    // Exporter 是非关键冷路径：它不参与主 select，意外退出不得关闭 server。
    if let Some(shutdown) = training_exporter_shutdown {
        if let Err(error) = shutdown.shutdown_tx.send(true) {
            warn!("训练导出关闭信号发送失败（task 可能已退出）: {error:?}");
        }
        match tokio::time::timeout(TRAINING_EXPORT_SHUTDOWN_TIMEOUT, shutdown.handle).await {
            Ok(Ok(())) => info!("训练导出 task 已优雅退出"),
            Ok(Err(error)) => error!("训练导出 task join 失败: {error}"),
            Err(_) => warn!(
                timeout_secs = TRAINING_EXPORT_SHUTDOWN_TIMEOUT.as_secs(),
                "训练导出 task 未在 timeout 内退出, 继续主流程"
            ),
        }
    }

    info!("服务停止");
    Ok(())
}
