// ============================================================================
// OpenClaw Cyber-Jianghu 数据驱动配置模块
// ============================================================================
//
// 本模块实现数据驱动架构，允许游戏配置通过 JSON 文件管理
//
// 目录结构:
// - types.rs: 配置数据结构定义
// - loader.rs: JSON 文件加载器
// - cache.rs: 运行时缓存
//
// 配置文件目录: crates/server/config/
// - game_rules.json: 游戏规则（初始状态、衰减率等）
// - items.json: 物品定义
// - actions.json: 行动配置
// - initial_inventory.json: 初始物品清单
// ============================================================================

mod cache;
#[allow(unused)]
mod formula_engine;
mod loader;
pub mod loaders;
pub mod registry;
pub mod types;

#[cfg(test)]
mod test_utils;

pub use cache::GameDataCache;
pub use loader::load_from_dir;
pub use registry::{
    ActionField, ActionRegistry, InitialInventoryRegistry, InventoryRegistry, NetworkRegistry,
    StateRegistry, init_registry, registry, registry_or_panic,
};
pub use types::{ActionEffect, ActionRequirement, GameData, ItemConfigEntry, ItemEffect};

#[cfg(test)]
pub use test_utils::init_test_registry;

// ============================================================================
// 便捷函数
// ============================================================================

/// 从默认配置目录加载所有游戏数据
///
/// 支持以下场景（按优先级顺序尝试）：
/// 1. 环境变量 `CYBER_JIANGHU_CONFIG_DIR`
/// 2. `config/` - Docker 容器内（配置文件被复制到 /app/config）
/// 3. `crates/server/config/` - 本地开发（从项目根目录运行）
///
/// # 返回
/// 返回完整游戏数据，或返回错误
pub fn load_game_data() -> anyhow::Result<GameData> {
    load_from_dir(crate::paths::get_config_dir())
}
