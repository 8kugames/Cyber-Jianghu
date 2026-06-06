// ============================================================================
// 身份系统（人设 + 事件演化）
// ============================================================================

pub mod dynamic_persona;
pub mod event_mapper;
pub mod trait_types;

pub use dynamic_persona::{DynamicPersona, PersonaState, ThreadSafePersona};
pub use event_mapper::{EventTraitMapper, TraitMappingRule};
pub use trait_types::{Trait, TraitChange, TraitType};
