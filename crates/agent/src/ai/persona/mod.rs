// ============================================================================
// 动态人设模块
// ============================================================================
//
// 实现运行时可修改的人设系统
// - DynamicPersona: 运行时可变的人设
// - Trait: 可演化的性格特质
// - EventTraitMapper: 事件到特质变化的映射
//
// 核心设计理念：
// - 人设从静态配置转为运行时可修改
// - 特质值随事件动态变化
// - 支持人设演化和历史追踪
// ============================================================================

pub mod dynamic_persona;
pub mod event_mapper;
pub mod trait_types;

// 重新导出核心类型
pub use dynamic_persona::{DynamicPersona, PersonaState, ThreadSafePersona};
pub use event_mapper::{EventTraitMapper, TraitMappingRule};
pub use trait_types::{Trait, TraitChange, TraitType};
