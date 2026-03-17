// ============================================================================
// OpenClaw Cyber-Jianghu 网络配置访问器
// ============================================================================

use super::global::registry;
use crate::game_data::types::unified_config::WebSocketConfigData;

/// 网络配置访问器
pub struct NetworkRegistry;

impl NetworkRegistry {
    /// 获取 WebSocket 配置
    pub fn websocket() -> WebSocketConfigData {
        registry()
            .map(|r| r.get().network.data.websocket.clone())
            .expect("配置未初始化，请确保 network.json 已正确加载")
    }
}
