// ============================================================================
// OpenClaw Cyber-Jianghu 全局配置注册表
// ============================================================================

use crate::game_data::GameDataCache;
use std::sync::{Arc, OnceLock};

/// 全局配置注册表
///
/// 使用 OnceLock 保证线程安全的单次初始化
static CONFIG_REGISTRY: OnceLock<Arc<GameDataCache>> = OnceLock::new();

/// 初始化全局配置注册表
///
/// 应该在服务器启动时调用一次
pub fn init_registry(cache: Arc<GameDataCache>) {
    let _ = CONFIG_REGISTRY.set(cache);
}

/// 获取全局配置注册表
///
/// 返回 GameDataCache 的只读引用
pub fn registry() -> Option<&'static GameDataCache> {
    CONFIG_REGISTRY.get().map(|arc| arc.as_ref())
}

/// 获取全局配置注册表（Result 版本）
///
/// 如果注册表未初始化返回错误，避免运行时 panic
pub fn registry_or_error() -> Result<&'static GameDataCache, String> {
    CONFIG_REGISTRY
        .get()
        .map(|arc| arc.as_ref())
        .ok_or_else(|| "CONFIG_REGISTRY 未初始化，请先调用 init_registry()".to_string())
}

/// 重置全局配置注册表（仅用于测试）
///
/// 测试中调用此函数来重置注册表，避免测试隔离性问题
#[cfg(test)]
#[allow(dead_code)]
pub fn reset_registry_for_test(cache: Arc<GameDataCache>) {
    let _ = CONFIG_REGISTRY.set(cache);
}
