// ============================================================================
// OpenClaw Cyber-Jianghu 物品系统
// ============================================================================
//
// 本模块实现数据驱动的物品系统，包括：
// - 物品定义和模板
// - 物品效果应用
// - 从配置文件加载物品
//
// 设计原则：
// 1. 物品定义完全数据驱动（从 JSON 配置加载）
// 2. 物品效果直接应用到Agent状态
// 3. 清晰的错误处理
// 4. 详细的中文注释
//
// 模块结构：
// - types: 物品类型定义
// - registry: 物品注册表和缓存
// - system: 物品效果应用逻辑
// ============================================================================

pub(crate) mod registry;
pub(crate) mod system;
pub(crate) mod types;

#[cfg(test)]
mod tests;

pub use registry::{get_item_definition, init_item_cache_from_config};
#[allow(unused_imports)]
pub use system::apply_item_effect;
