// ============================================================================
// OpenClaw Cyber-Jianghu 网络配置访问器
// ============================================================================

use super::global::registry;
use crate::game_data::types::unified_config::{DeviceRegisterConfigData, WebSocketConfigData};

/// 网络配置访问器
pub struct NetworkRegistry;

impl NetworkRegistry {
    /// 获取 WebSocket 配置
    pub fn websocket() -> WebSocketConfigData {
        registry()
            .map(|r| r.get().network.data.websocket.clone())
            .expect("配置未初始化，请确保 network.json 已正确加载")
    }

    /// 获取设备注册配置
    pub fn device_register() -> DeviceRegisterConfigData {
        registry()
            .map(|r| r.get().network.data.device_register.clone())
            .expect("配置未初始化，请确保 network.json 已正确加载")
    }
}
