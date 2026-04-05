// ============================================================================
// 身份系统（人设 + 寿命 + 事件演化）
// ============================================================================

pub mod dynamic_persona;
pub mod event_mapper;
pub mod lifespan;
pub mod lifespan_types;
pub mod prompts;
pub mod trait_types;

pub use dynamic_persona::{DynamicPersona, PersonaState, ThreadSafePersona};
pub use event_mapper::{EventTraitMapper, TraitMappingRule};
pub use lifespan::LifespanCalculator;
pub use lifespan_types::{
    AgingEffectValues, AgingEffects, AgingStage, LifespanConfig, LifespanStatus,
};
pub use prompts::{AgentPrompt, get_agent_prompt, get_all_agent_prompts};
pub use trait_types::{Trait, TraitChange, TraitType};
