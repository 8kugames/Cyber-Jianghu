// ============================================================================
// 身份系统（人设 + 事件演化）
// ============================================================================

pub mod dynamic_persona;
pub mod event_mapper;
pub mod prompts;
pub mod trait_types;

pub use dynamic_persona::{DynamicPersona, PersonaState, ThreadSafePersona};
pub use event_mapper::{EventTraitMapper, TraitMappingRule};
pub use prompts::{AgentPrompt, get_agent_prompt, get_all_agent_prompts};
pub use trait_types::{Trait, TraitChange, TraitType};
