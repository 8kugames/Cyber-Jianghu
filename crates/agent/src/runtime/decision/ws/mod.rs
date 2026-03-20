// ============================================================================
// WebSocket 决策模块
// ============================================================================
//
// 提供 Agent 与外部调度器（OpenClaw）之间的 WebSocket 实时通信
//
// 协议：
// - tick: 推送 WorldState + 截止时间
// - tick_closed: 超时通知
// - intent: 提交意图
//
// 使用方式：
// 1. 创建 WsDecisionState
// 2. 启动 WebSocket + HTTP 混合服务
// 3. 在主循环中调用 broadcast_tick() 和 recv_intent()
// ============================================================================

pub mod protocol;
pub mod server;
pub mod state;

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use tracing::{info, warn};
use uuid::Uuid;

use cyber_jianghu_protocol::{Intent, WorldState};

pub use protocol::{DownstreamMessage, UpstreamMessage, WsIntent};
pub use server::{run_ws_server, ws_router};
pub use state::{
    ws_intent_to_intent, WsDecisionState, WsSharedState, DEFAULT_TICK_DURATION_SECS,
    TICK_TIMEOUT_RATIO,
};

// ============================================================================
// 决策函数
// ============================================================================

/// 创建 WebSocket 决策函数
///
/// 工作流程：
/// 1. 收到 WorldState 后广播给 WebSocket 客户端
/// 2. 等待外部通过 WebSocket 提交 Intent
/// 3. 超时返回 idle 意图，并通知客户端
pub fn ws_decision(
    agent_id: Arc<tokio::sync::RwLock<Uuid>>,
    state: Arc<tokio::sync::Mutex<WsDecisionState>>,
    tick_duration_secs: u64,
) -> impl Fn(&WorldState) -> BoxFuture<'static, Intent> + Send + Sync + 'static {
    move |world_state: &WorldState| {
        let world_state = world_state.clone();
        let state = state.clone();
        let agent_id_clone = agent_id.clone();

        Box::pin(async move {
            // 计算截止时间（tick_duration * 0.9）
            let timeout_ratio = TICK_TIMEOUT_RATIO;
            let timeout_secs = (tick_duration_secs as f64 * timeout_ratio) as u64;
            let deadline = Instant::now() + Duration::from_secs(timeout_secs);

            // 获取 agent_id
            let agent_id_value = *agent_id_clone.read().await;

            // 获取状态锁
            let mut ws_state = state.lock().await;

            // 1. 广播 Tick 给 WebSocket 客户端
            ws_state.broadcast_tick(&world_state, deadline);

            // 2. 等待外部决策
            match ws_state.recv_intent(deadline).await {
                Some(intent) => {
                    // 收到有效 Intent
                    info!(
                        "Received intent for tick {}: {}",
                        world_state.tick_id, intent.action_type
                    );
                    ws_intent_to_intent(intent, agent_id_value, world_state.tick_id)
                }
                None => {
                    // 超时或过期，自动 idle
                    warn!(
                        "Tick {} timeout or expired, auto idle",
                        world_state.tick_id
                    );

                    // TODO: 发送 tick_closed 消息给客户端
                    // 这需要在 state 中添加一个专门的 broadcast channel

                    Intent::idle(agent_id_value, world_state.tick_id)
                        .with_thought("Tick timeout, auto idle".to_string())
                }
            }
        })
    }
}

// ============================================================================
// 辅助类型
// ============================================================================

/// WebSocket 决策配置
#[derive(Debug, Clone)]
pub struct WsDecisionConfig {
    /// 监听端口（0 = 在 23340-23349 范围内选择）
    pub port: u16,
    /// Tick 持续时间（秒）
    pub tick_duration_secs: u64,
}

impl Default for WsDecisionConfig {
    fn default() -> Self {
        Self {
            port: 0,
            tick_duration_secs: DEFAULT_TICK_DURATION_SECS,
        }
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_decision_config_default() {
        let config = WsDecisionConfig::default();
        assert_eq!(config.port, 0);
        assert_eq!(config.tick_duration_secs, 60);
    }
}
