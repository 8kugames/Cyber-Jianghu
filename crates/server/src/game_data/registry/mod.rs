// ============================================================================
// OpenClaw Cyber-Jianghu 统一配置注册表
// ============================================================================
//
// 本模块实现 COI (组合优于继承) 原则：
// - 所有游戏配置通过数据文件定义（JSON/YAML）
// - 代码只提供访问器和验证逻辑
// - 配置热加载通过重新加载 GameDataCache 实现
//
// 配置来源：
// - config/game-rules.json      - 游戏规则
// - config/actions.json         - 动作定义
// - config/attributes.json      - 属性系统
// - config/initial-inventory.json - 初始物品
// - config/locations.json       - 位置图
// ============================================================================

mod action_registry;
mod chronicle_registry;
mod global;
mod initial_recipes_registry;
mod inventory_registry;
mod item_registry;
mod network_registry;
pub mod recipe_registry;
pub mod skill_registry;
mod state_registry;
pub mod time_registry;

pub use action_registry::{ActionField, ActionRegistry};
pub use chronicle_registry::ChronicleRegistry;
pub use global::{init_registry, registry, registry_or_error};
pub use initial_recipes_registry::InitialRecipesRegistry;
pub use inventory_registry::{InitialInventoryRegistry, InventoryRegistry};
pub use item_registry::ItemRegistry;
pub use network_registry::NetworkRegistry;
pub use recipe_registry::RecipeRegistry;
pub use skill_registry::SkillRegistry;
pub use state_registry::StateRegistry;
pub use time_registry::TimeRegistry;

// Re-export LocationRegistry from cache module
