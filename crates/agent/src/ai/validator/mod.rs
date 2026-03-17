// ============================================================================
// 意图验证器模块
// ============================================================================

pub mod cognitive_validator;
pub mod engine;
pub mod rule_engine;
mod prompt;
mod types;

pub use cognitive_validator::{CognitiveEngineWithRetry, CognitiveValidationResult, CognitiveValidator};
pub use types::{
    LlmValidationResponse, PersonaInfo, RejectionType, ValidationRequest, ValidationResult,
};
pub use rule_engine::{
    Rule, RuleCondition, RuleEngine, RuleEngineConfig, RuleEngineValidator, RuleType,
    RuleValidationContext, RuleValidationResult,
};

pub use engine::{IntentValidator, Validator};
pub use prompt::{sanitize_for_prompt, ObserverPrompt};
