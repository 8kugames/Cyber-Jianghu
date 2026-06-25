// ============================================================================
// WebSocket Server 实现
// ============================================================================
//
// 提供 Agent 与外部调度器（OpenClaw）之间的 WebSocket 通信
// 同时保留 HTTP API 用于数据访问
//
// 安全限制：
// - 仅允许 localhost 连接
// - 每个 Agent 只允许一个 OpenClaw 连接
// ============================================================================

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::{
    Router,
    extract::{
        ConnectInfo, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::Response,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, error, info, warn};

use super::protocol::{DownstreamMessage, ServerErrorCode, UpstreamMessage, WsIntent};
use super::state::WsSharedState;
use crate::infra::api::{HttpApiState, create_api_router, get_static_serve_dir};

// ============================================================================
// WebSocket 路由
// ============================================================================

/// 创建 WebSocket 路由
pub fn ws_router(shared_state: WsSharedState) -> Router<WsSharedState> {
    Router::new()
        .route("/ws", get(ws_handler))
        .with_state(shared_state)
}

/// WebSocket 处理器（带 localhost 限制和单连接限制）
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<WsSharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    // 安全检查：仅允许 localhost 连接，除非配置允许外部连接
    if !addr.ip().is_loopback() && !state.allow_external_connections {
        warn!(
            "Rejected WebSocket connection from non-localhost: {} (allow_external_connections=false)",
            addr
        );
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body("Only localhost connections allowed (set CYBER_JIANGHU_WS_ALLOW_EXTERNAL=1 to allow)".into())
            .expect("valid HTTP response");
    }

    // 安全：单连接限制
    if state.openclaw_connected.swap(true, Ordering::Acquire) {
        warn!("Rejected second WebSocket connection (only one allowed)");
        // 注意：不要在这里 store(false)，因为：
        // 1. swap(true) 返回 true 说明已经有连接
        // 2. 我们没有成功建立连接，所以不应该释放 slot
        // 3. slot 由已建立的连接在断开时释放
        return Response::builder()
            .status(StatusCode::CONFLICT)
            .body("Only one OpenClaw connection allowed".into())
            .expect("valid HTTP response");
    }

    debug!("WebSocket connection request from {}", addr);
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

// ============================================================================
// WebSocket 连接处理
// ============================================================================

/// 处理 WebSocket 连接
async fn handle_socket(socket: WebSocket, state: WsSharedState) {
    info!("OpenClaw WebSocket client connected");

    let (ws_tx, ws_rx) = socket.split();
    let ws_tx = Arc::new(Mutex::new(ws_tx));

    // 订阅 WorldState 广播
    let mut state_rx = state.state_tx.subscribe();
    // 订阅 tick_closed 广播
    let mut tick_closed_rx = state.tick_closed_tx.subscribe();
    // 订阅 Server 消息广播（用于透传）
    let mut server_msg_rx = state.server_msg_tx.subscribe();
    let _intent_tx = state.intent_tx.clone();

    // 获取上行消息接收通道（用于转发 OpenClawBridge 的 LLM 请求）
    let upstream_rx = state.upstream_rx.lock().expect("lock poisoned").take();
    let llm_response_tx = state.llm_response_tx.clone();

    // 使用 Arc<AtomicBool> 来共享活跃状态
    let is_active = Arc::new(AtomicBool::new(true));
    let is_active_read = is_active.clone();
    let is_active_write = is_active.clone();

    // 读任务：接收客户端消息
    let read_task = async {
        let mut ws_rx = ws_rx;
        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    debug!("Received message");

                    if let Ok(DownstreamMessage::LLMResponse {
                        request_id,
                        content,
                        error,
                    }) = serde_json::from_str::<DownstreamMessage>(&text)
                    {
                        let result = if let Some(err) = error {
                            Err(err)
                        } else {
                            Ok(content)
                        };
                        if let Err(e) = llm_response_tx.send((request_id, result)).await {
                            warn!("Failed to send LLM response to bridge: {}", e);
                        }
                        continue;
                    }

                    match serde_json::from_str::<UpstreamMessage>(&text) {
                        Ok(upstream) => {
                            // 统一认知模式：外部 Intent 提交已被禁用
                            // 三魂管道内部处理所有 intent 生成，仅保留 LLMRequest 通道
                            let intent_opt: Option<WsIntent> = upstream.into();
                            if let Some(intent) = intent_opt {
                                let current_tick = state.get_current_tick();
                                let error_msg = DownstreamMessage::ServerError {
                                    code: ServerErrorCode::InvalidAction,
                                    message: "Unified cognitive mode: intents generated internally"
                                        .to_string(),
                                    tick_id: Some(intent.tick_id),
                                    current_tick: Some(current_tick),
                                };

                                if let Ok(json) = serde_json::to_string(&error_msg) {
                                    let mut tx = ws_tx.lock().await;
                                    if let Err(e) = tx.send(Message::Text(json.into())).await {
                                        tracing::warn!("claw ws_tx.send(error) 失败（receiver 可能已 drop）：{e:?}");
                                    }
                                }

                                warn!(
                                    "Rejected external intent in unified mode: tick {}",
                                    intent.tick_id
                                );
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse message: {}", e);
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    info!("Client requested close");
                    break;
                }
                Ok(Message::Ping(data)) => {
                    debug!("Ping received");
                    let mut tx = ws_tx.lock().await;
                    if let Err(e) = tx.send(Message::Pong(data)).await {
                        tracing::warn!("claw ws_tx.send(Pong) 失败（receiver 可能已 drop）：{e:?}");
                    }
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }
        is_active_read.store(false, Ordering::Relaxed);
    };

    // 写任务：广播 WorldState 和 tick_closed
    let write_task = async {
        let mut upstream_rx = upstream_rx;

        loop {
            if !is_active_write.load(Ordering::Relaxed) {
                break;
            }

            // 使用 select 同时监听多个通道
            tokio::select! {
                // 接收 WorldState
                result = state_rx.recv() => {
                    match result {
                        Ok(world_state) => {
                            // 构造 tick 消息
                            // 生成叙事化上下文
                            let context = state.generate_context(&world_state);
                            // 生成认知上下文
                            let cognitive_context = state.generate_cognitive_context(&world_state);
                            let msg = DownstreamMessage::Tick {
                                tick_id: world_state.tick_id,
                                state: (*world_state).clone(),
                                context,
                                cognitive_context,
                            };

                            let json = match serde_json::to_string(&msg) {
                                Ok(j) => j,
                                Err(e) => {
                                    error!("Failed to serialize tick message: {}", e);
                                    continue;
                                }
                            };

                            let mut tx = ws_tx.lock().await;
                            if let Err(e) = tx.send(Message::Text(json.into())).await {
                                debug!("Failed to send tick: {}", e);
                                break;
                            }

                            debug!("Sent tick {}", world_state.tick_id);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            debug!("Broadcast channel closed");
                            break;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Client lagged {} messages", n);
                        }
                    }
                }
                // 接收 tick_closed
                result = tick_closed_rx.recv() => {
                    match result {
                        Ok(msg) => {
                            let json = match serde_json::to_string(&msg) {
                                Ok(j) => j,
                                Err(e) => {
                                    error!("Failed to serialize tick_closed message: {}", e);
                                    continue;
                                }
                            };

                            let mut tx = ws_tx.lock().await;
                            if let Err(e) = tx.send(Message::Text(json.into())).await {
                                debug!("Failed to send tick_closed: {}", e);
                                break;
                            }

                            debug!("Sent tick_closed message");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            debug!("tick_closed channel closed");
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Client lagged {} tick_closed messages", n);
                        }
                    }
                }
                // 接收 Server 消息透传
                result = server_msg_rx.recv() => {
                    match result {
                        Ok(msg) => {
                            let json = match serde_json::to_string(&msg) {
                                Ok(j) => j,
                                Err(e) => {
                                    error!("Failed to serialize server message: {}", e);
                                    continue;
                                }
                            };

                            let mut tx = ws_tx.lock().await;
                            if let Err(e) = tx.send(Message::Text(json.into())).await {
                                debug!("Failed to send server message: {}", e);
                                break;
                            }

                            debug!("Sent server message to OpenClaw");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            debug!("server_msg channel closed");
                            break;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Client lagged {} server messages", n);

                            // 发送 MissedMessages 通知
                            let missed_msg = DownstreamMessage::MissedMessages {
                                count: n,
                                suggest_resync: n > 5,
                            };
                            if let Ok(json) = serde_json::to_string(&missed_msg) {
                                let mut tx = ws_tx.lock().await;
                                if let Err(e) = tx.send(Message::Text(json.into())).await {
                                    tracing::warn!("claw ws_tx.send(missed_msg) 失败（receiver 可能已 drop）：{e:?}");
                                }
                            }
                        }
                    }
                }
                // 接收来自 OpenClawBridge 的上行消息（转发到 WebSocket）
                Some(msg) = async {
                    if let Some(ref mut rx) = upstream_rx {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    match serde_json::to_string(&msg) {
                        Ok(json) => {
                            let mut tx = ws_tx.lock().await;
                            if let Err(e) = tx.send(Message::Text(json.into())).await {
                                debug!("Failed to send upstream message: {}", e);
                                break;
                            }
                            debug!("Forwarded upstream message to OpenClaw");
                        }
                        Err(e) => {
                            error!("Failed to serialize upstream message: {}", e);
                        }
                    }
                }
            }
        }
    };

    // 并行运行读写任务
    tokio::select! {
        _ = read_task => debug!("Read task ended"),
        _ = write_task => debug!("Write task ended"),
    }

    // 重置连接标志
    state.openclaw_connected.store(false, Ordering::Release);
    debug!("OpenClaw connection slot released");

    info!("OpenClaw WebSocket client disconnected");
}

// ============================================================================
// 启动混合服务器（WebSocket + HTTP API）
// ============================================================================

/// 启动混合服务器（claw 模式）
///
/// 监听指定端口，同时提供：
/// - WebSocket `/ws` 用于实时决策
/// - HTTP API `/api/v1/*` 用于数据访问
/// - 静态文件服务 `/panel` 用于 Web 面板
///
/// 安全限制：
/// - WebSocket 仅接受 localhost 连接
/// - 每个 Agent 只允许一个 OpenClaw WebSocket 连接
pub async fn run_ws_server(
    port: u16,
    ws_state: WsSharedState,
    api_state: HttpApiState,
) -> anyhow::Result<()> {
    use std::net::SocketAddr;

    // WebSocket 路由
    let ws_router = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(ws_state);

    // HTTP API 路由（复用 http 模块）
    // P0-11(b)：API 端点必须携带 device auth_token（与 run_http_server 一致）
    let api_router = create_api_router()
        .layer(axum::middleware::from_fn_with_state(
            api_state.clone(),
            crate::infra::api::auth::require_device_token,
        ))
        .with_state(api_state);

    // 合并路由
    let app = Router::new().merge(ws_router).merge(api_router);

    // 添加静态文件服务（用于 Web 面板）
    let serve_dir = get_static_serve_dir();
    let app = app.fallback_service(tower_http::services::ServeDir::new(serve_dir));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;

    info!("[claw] Mixed Server listening on {}", local_addr);
    info!("[claw] HTTP_PORT={}", local_addr.port());
    info!(
        "[claw] WebSocket: ws://127.0.0.1:{}/ws (localhost only)",
        local_addr.port()
    );
    info!(
        "[claw] HTTP API: http://127.0.0.1:{}/api/v1",
        local_addr.port()
    );

    // 使用 into_make_service_with_connect_info 来获取客户端地址
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
