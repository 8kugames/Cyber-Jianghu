// ============================================================================
// OpenClaw Cyber-Jianghu 配置加载器子模块
// ============================================================================
//
// 本模块包含各个配置类型的专用加载器
// ============================================================================

mod actions_loader;
mod attributes_loader;
mod game_rules_loader;
mod inventory_loader;
mod items_loader;
mod locations_loader;
mod narrative_loader;
mod network_loader;
mod recipes_loader;
mod time_loader;

// 测试工具模块（仅测试时可用，但仍需pub以便其他测试使用）
#[cfg(test)]
pub mod test_utils;

// 重导出所有加载器函数
pub use actions_loader::load_actions;
pub use attributes_loader::load_attributes;
pub use game_rules_loader::load_game_rules;
pub use inventory_loader::{load_initial_inventory, load_inventory};
pub use items_loader::load_items;
pub use locations_loader::load_locations;
pub use narrative_loader::load_narrative;
pub use network_loader::load_network;
pub use recipes_loader::load_recipes;
pub use time_loader::load_time;
