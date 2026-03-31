// ============================================================================
// Claw Runtime - OpenClaw Bridge + WebSocket Server
// ============================================================================
//
// 提供 Claw 模式的 OpenClaw 集成和 WebSocket 服务

mod bridge;
mod protocol;
mod server;
pub mod state;
mod validation;

pub use bridge::{BridgeConfig, LlmClientContainer, OpenClawBridge};
pub use protocol::{DownstreamMessage, UpstreamMessage, WsIntent};
pub use server::{run_ws_server, ws_router};
pub use state::{
    DEFAULT_TICK_DURATION_SECS, TICK_TIMEOUT_RATIO, WsDecisionState, WsSharedState,
    ws_intent_to_intent,
};
pub use validation::{ValidationTaskParams, WsValidationRequest, spawn_validation_task};
