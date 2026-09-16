//! LLM 客户端工厂与端口/HTTP/Claw 服务器启动辅助

use super::*;

/// 创建 LLM 客户端
/// - Cognitive: 内置 FallbackLlmClient
/// - Claw: OpenClawBridge (外部 OpenClaw 调度器)
///   其他一切 agent 能力（记忆、关系、三魂）都应统一，不因模式而异
pub(crate) fn create_llm_client(
    runtime_mode: RuntimeMode,
    config: &Config,
    shared_state: Option<Arc<WsSharedState>>,
) -> Result<Arc<dyn cyber_jianghu_agent::component::llm::LlmClient>> {
    match runtime_mode {
        RuntimeMode::Cognitive => Ok(cyber_jianghu_agent::component::llm::build_fallback_client(
            &config.llm,
            config.llm.enable_streaming,
            Some(config.earth_soul.clone()),
        )?),
        RuntimeMode::Claw => {
            let upstream_tx = shared_state
                .expect("Claw mode needs shared_state")
                .upstream_tx
                .clone();
            let bridge = OpenClawBridge::new(upstream_tx, BridgeConfig::default());
            Ok(Arc::new(bridge) as Arc<dyn cyber_jianghu_agent::component::llm::LlmClient>)
        }
    }
}

// ============================================================================
// 等待角色创建
// ============================================================================

/// 检查端口是否可用（未被占用）
pub(crate) async fn is_port_available(port: u16) -> bool {
    tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .is_ok()
}

/// 自动选择可用端口，优先 23340
pub(crate) async fn pick_auto_port() -> u16 {
    const PREFERRED_PORT: u16 = 23340;
    const PORT_RANGE_START: u16 = 23340;
    const PORT_RANGE_END: u16 = 23999;

    // 优先尝试 23340
    if is_port_available(PREFERRED_PORT).await {
        info!("使用首选端口: {}", PREFERRED_PORT);
        return PREFERRED_PORT;
    }

    // 23340 被占用，随机选择其他端口
    use rand::RngExt;
    let mut rng = rand::rng();
    let available_ports: Vec<u16> = (PORT_RANGE_START..=PORT_RANGE_END)
        .filter(|&p| p != PREFERRED_PORT)
        .collect();

    // 随机打乱可用端口
    let random_idx = rng.random_range(0..available_ports.len());
    let selected_port = available_ports[random_idx];
    info!(
        "首选端口 {} 被占用，选择端口: {} (范围: {}-{}, 已排除 {})",
        PREFERRED_PORT, selected_port, PORT_RANGE_START, PORT_RANGE_END, PREFERRED_PORT
    );
    selected_port
}

pub(crate) fn start_claw_server(
    port: u16,
    runtime_agent_id: Arc<RwLock<Uuid>>,
    config: &Config,
    ws_url: &str,
    device: &DeviceConfig,
    server_dir: PathBuf,
) -> Result<ServerSetup> {
    let actual_port = if port == 0 {
        // 使用同步阻塞方式等待端口选择（避免 async trait 复杂化）
        tokio::runtime::Handle::current().block_on(pick_auto_port())
    } else {
        port
    };

    info!(
        "启动 Claw 模式（WebSocket + HTTP API），端口: {}",
        actual_port
    );

    let config_path_str = config_path().display().to_string();
    print_startup_banner(actual_port, ws_url, &config_path_str, "Claw");

    let (reconnect_tx, _) =
        tokio::sync::broadcast::channel::<cyber_jianghu_agent::infra::api::ReconnectRequest>(64);

    let ws_state = WsDecisionState::new();
    let shared_state = Arc::new(WsSharedState::from(&ws_state));
    // 统一认知模式下外部 Intent 已被 server.rs 拦截，无需启动验证任务
    // CAS 去重逻辑保留在 WsDecisionState 中作为通用安全机制
    // ws_state.spawn_validation_task((*shared_state).clone());

    // Derive HTTP URL from WS URL
    let http_url = cyber_jianghu_agent::config::ws_to_http_url(ws_url);

    let character_dir = server_dir.join("characters");
    let (_http_decision_state, api_state) = create_http_state(
        runtime_agent_id,
        http_url.to_string(),
        ws_url.to_string(),
        Some(device.clone()),
        server_dir,
        character_dir,
        Some(reconnect_tx),
        config_path(),
        Some(shared_state.clone()),
        config.runtime.mode,
        actual_port,
    );

    let api_state_clone = api_state.clone();
    let server_msg_tx = shared_state.server_msg_tx.clone();
    let shared_state_for_callback = shared_state.clone();
    let api_state_for_callback = api_state.clone();
    tokio::spawn(async move {
        if let Err(e) = run_ws_server(actual_port, (*shared_state).clone(), api_state_clone).await {
            error!("Claw server error: {}", e);
        }
    });

    Ok(ServerSetup {
        server_msg_tx,
        shared_state: shared_state_for_callback,
        api_state: Arc::new(api_state_for_callback),
        actual_port,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_http_api_server(
    port: u16,
    runtime_agent_id: Arc<RwLock<Uuid>>,
    config: &Config,
    ws_url: &str,
    device: &DeviceConfig,
    server_dir: PathBuf,
    reconnect_tx: Option<
        tokio::sync::broadcast::Sender<cyber_jianghu_agent::infra::api::ReconnectRequest>,
    >,
) -> Result<(Arc<cyber_jianghu_agent::infra::api::HttpApiState>, u16)> {
    let port_range_start = 23340u16;
    let port_range_end = 23999u16;

    let actual_port = if port == 0 {
        pick_auto_port().await
    } else {
        port
    };

    info!("启动 HTTP API 服务器，端口: {}", actual_port);

    let config_path_str = config_path().display().to_string();
    print_startup_banner(actual_port, ws_url, &config_path_str, "Cognitive");

    // Derive HTTP URL from WS URL
    let http_url = cyber_jianghu_agent::config::ws_to_http_url(ws_url);

    let character_dir = server_dir.join("characters");
    let (_http_decision_state, api_state) = cyber_jianghu_agent::runtime::create_http_state(
        runtime_agent_id,
        http_url.to_string(),
        ws_url.to_string(),
        Some(device.clone()),
        server_dir,
        character_dir,
        reconnect_tx,
        config_path(),
        None,
        config.runtime.mode,
        actual_port,
    );

    let api_state_clone = api_state.clone();
    let is_auto_port = port == 0;
    let resolved_port = Arc::new(tokio::sync::Mutex::new(actual_port));
    let resolved_port_clone = resolved_port.clone();
    tokio::spawn(async move {
        let mut try_port = actual_port;

        loop {
            match cyber_jianghu_agent::runtime::run_http_server(try_port, api_state_clone.clone())
                .await
            {
                Ok(()) => return,
                Err(e) if is_auto_port => {
                    let mut next = try_port + 1;
                    if next > port_range_end {
                        next = port_range_start;
                    }
                    if next == actual_port {
                        error!(
                            "HTTP API server error: 所有端口 {}-{} 均被占用: {}",
                            port_range_start, port_range_end, e
                        );
                        return;
                    }
                    warn!(
                        "HTTP API server error: 端口 {} 被占用 ({})，尝试端口 {}",
                        try_port, e, next
                    );
                    *resolved_port_clone.lock().await = next;
                    try_port = next;
                }
                Err(e) => {
                    error!("HTTP API server error: {}", e);
                    return;
                }
            }
        }
    });

    let final_port = actual_port;
    Ok((Arc::new(api_state), final_port))
}
