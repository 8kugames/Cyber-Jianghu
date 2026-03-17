// ============================================================================
// OpenClaw Cyber-Jianghu 数据驱动配置类型定义
// ============================================================================
//
// 本模块导出所有类型定义，拆分为多个子模块
// ============================================================================

// 重新导出子模块
pub mod actions;
pub mod attribute_component;
pub mod attributes;
pub mod attributes_config;
pub mod components;
pub mod derived_component;
pub mod game_data;
pub mod game_rules;
pub mod inventory;
pub mod inventory_config;
pub mod items;
pub mod locations;
pub mod primary_attributes;
pub mod status_component;
pub mod unified_attributes;
pub mod validation;
pub mod recipes;
pub mod unified_config;

// 重新导出统一配置类型（新格式）
pub use unified_config::*;
pub use unified_attributes::*;
pub use game_data::*;

// 重新导出各个数据结构体（用于统一配置的 data 部分）
pub use actions::*;
pub use items::*;
pub use inventory::*;
pub use recipes::*;

// 重新导出组件类型
pub use attribute_component::*;
pub use status_component::*;
pub use components::*;

// 重新导出其他必要的类型
pub use attributes::*;
