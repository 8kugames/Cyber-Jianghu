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
    ActionEvolutionConfig, CapabilityManifest, ProposalStore, SoulReviewEngine, TopicClassifier,
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
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use tokio::task::JoinHandle;
use tracing::{Level, error, info, warn};
use tracing_subscriber::FmtSubscriber;

// ============================================================================
// Tick引擎启动
// ============================================================================

fn serve_admin_file(path: &str) -> Result<axum::response::Response<Body>, StatusCode> {
    let static_dir = crate::paths::get_static_dir().join("admin");

    let file_path = if path.is_empty() || path == "index.html" {
        static_dir.join("index.html")
    } else {
        static_dir.join(path)
    };

    if !file_path.exists() || !file_path.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }

    let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
    let body =
        Body::from(std::fs::read(&file_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?);
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, mime.as_ref())
        .body(body)
        .unwrap_or_else(|_| axum::response::Response::new(Body::empty())))
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

/// 初始化治理系统
///
/// 加载 action_evolution.yaml 配置，创建 TopicClassifier、ProposalStore、SoulReviewEngine，
/// 并启动周期审议后台任务。
async fn init_governance(
    db_pool: &DbPool,
    connection_manager: websocket::ConnectionManager,
) -> Result<GovernanceState> {
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

    let manifest = CapabilityManifest::load();
    info!(
        "CapabilityManifest 加载完成: {} 条目",
        manifest.entries().len()
    );

    let classifier = Arc::new(TopicClassifier::new(action_evo_config.topic_classifier));
    let proposal_store = Arc::new(ProposalStore::new(db_pool.clone()));

    // SoulReviewEngine::load 接受 config_dir，内部加载 souls.yaml
    let engine =
        Arc::new(SoulReviewEngine::load(&config_dir).context("SoulReviewEngine 初始化失败")?);

    let review_config = engine.config().review.clone();

    // 启动周期审议任务
    let engine_clone = engine.clone();
    let store_clone = proposal_store.clone();
    let poll_interval = review_config.poll_interval_secs;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(poll_interval));
        loop {
            interval.tick().await;
            match store_clone.get_pending_groups().await {
                Ok(groups) if !groups.is_empty() => {
                    let results = engine_clone.review_pending(&store_clone, &groups).await;
                    for (group_id, status) in results {
                        info!("Group {} 审议完成: {}", group_id, status);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("获取待审议 groups 失败: {}", e);
                }
            }
        }
    });

    Ok(GovernanceState {
        manifest: Arc::new(tokio::sync::RwLock::new(manifest)),
        classifier,
        proposal_store,
        engine,
        connection_manager,
        review_config,
    })
}

// ============================================================================
// 主函数
// ============================================================================

#[tokio::main]
#[allow(clippy::await_holding_lock)]
async fn main() -> Result<()> {
    // 1. 初始化日志
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .with_thread_ids(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

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
    let db_pool = init_db_pool(
        &config.database.url,
        config.database.max_retries,
        config.database.retry_delay_secs,
    )
    .await?;
    info!("数据库连接池初始化成功");

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

    use std::fs::File;
    use std::io::Write;

    let token_path = crate::paths::get_logs_dir().join("cyber_jianghu_admin.tmp");
    if let Some(parent) = token_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    {
        let mut file = File::create(&token_path).map_err(|e| {
            anyhow::anyhow!("无法创建admin token文件 {}: {}", token_path.display(), e)
        })?;

        writeln!(file, "========================================")?;
        writeln!(file, "🔐 Cyber-Jianghu 管理员访问凭证")?;
        writeln!(file, "========================================")?;
        writeln!(file, "Read Token (只读): [{}]", read_token_source)?;
        writeln!(file, "  {}", admin_read_token)?;
        writeln!(file, "Write Token (读写): [{}]", write_token_source)?;
        writeln!(file, "  {}", admin_write_token)?;
        writeln!(file)?;
        writeln!(file, "========================================")?;
    }

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
    let governance = match init_governance(&db_pool, connection_manager.clone()).await {
        Ok(g) => {
            info!("治理系统初始化成功");
            Some(g)
        }
        Err(e) => {
            warn!("治理系统初始化失败（将继续运行，治理功能不可用）: {}", e);
            None
        }
    };

    // 9.3 创建应用状态
    let state = Arc::new(AppState::new(
        db_pool.clone(),
        connection_manager.clone(),
        agent_to_device_map.clone(),
        agent_state_cache.clone(),
        worker_tx.clone(),
        rate_limiter.clone(),
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
        db_pool,
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
        .route(
            "/api/config/llm",
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
            "/api/config/llm/status",
            get(handlers::config_llm::get_llm_status).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                handlers::auth::require_read_token,
            )),
        )
        .route(
            "/api/config/llm/enabled",
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
        // Action Evolution — 治理提案提交
        .route(
            "/api/v1/action-evolution/propose",
            post(crate::governance::handlers::submit_proposal),
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
    let shutdown_signal = async {
        // 监听 Ctrl+C (SIGINT)
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        // 监听 SIGTERM (Docker stop / Kubernetes pod termination)
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
    };

    // 14. 等待服务器结束、Tick引擎失败或关闭信号
    tokio::select! {
        // 关闭信号
        _ = shutdown_signal => {
            info!("正在关闭服务...");
            info!("服务已优雅关闭");
        }

        // Web服务器运行
        result = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()) => {
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
    }

    info!("服务停止");
    Ok(())
}
